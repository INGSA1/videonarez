//! Прокси для предпросмотра файлов, которые WebView2 не играет напрямую (MKV с H.264, AVI, ProRes…).
//!
//! Если видеокодек поддерживается браузером — делается только ремукс в MP4 (-c:v copy, секунды
//! на любой длине). Иначе — быстрое аппаратное перекодирование в 720p H.264.
//! Таймкоды прокси совпадают с оригиналом, а экспорт всегда идёт из оригинала.

use crate::config::gpu;
use crate::editor::encode::{video_args, Rate};
use crate::editor::probe;
use crate::error::{AppError, AppResult};
use crate::ffmpeg::{ffmpeg_cmd, run_progress, Priority};
use crate::state::AppState;
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProxyProgress {
    path: String,
    percent: f64,
}

fn cache_key(path: &Path) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        m.len().hash(&mut h);
        if let Ok(t) = m.modified() {
            t.hash(&mut h);
        }
    }
    format!("{:016x}", h.finish())
}

pub fn proxy_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let d = app
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::msg(e.to_string()))?
        .join("proxy");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

#[tauri::command]
pub async fn prepare_preview(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    transcode: bool,
) -> AppResult<String> {
    let src = PathBuf::from(&path);
    if !state.is_allowed(&src) {
        return Err(AppError::msg("Файл не открыт в редакторе"));
    }
    let dir = proxy_dir(&app)?;
    let out = dir.join(format!("{}{}.mp4", cache_key(&src), if transcode { "_t" } else { "" }));
    if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        state.allow(&out);
        return Ok(out.to_string_lossy().into_owned());
    }

    let meta = probe::probe(&path).await?;
    let token = state.analysis();
    let settings = state.settings();

    let copy_ok = !transcode
        && match meta.video_codec.as_str() {
            "h264" => matches!(meta.pix_fmt.as_str(), "yuv420p" | "yuvj420p"),
            "hevc" | "av1" | "vp9" => matches!(meta.pix_fmt.as_str(), "yuv420p" | "yuvj420p" | "yuv420p10le"),
            _ => false,
        };

    let tmp = out.with_extension("part");
    let mut cmd = ffmpeg_cmd(Priority::BelowNormal, true)?;
    if !copy_ok {
        cmd.args(["-hwaccel", "auto"]);
    }
    cmd.arg("-i").arg(&path);
    cmd.args(["-map", "0:v:0", "-map", "0:a:0?", "-sn", "-dn"]);
    if copy_ok {
        cmd.args(["-c:v", "copy"]);
    } else {
        let enc = gpu::pick(&settings.encoders, "auto", "h264")
            .ok_or_else(|| AppError::msg("Нет кодировщика H.264"))?;
        let fmt = crate::editor::encode::pix_fmt(&enc);
        cmd.args(["-vf", &format!("scale=-2:'min(720,ih)':flags=fast_bilinear,format={fmt}")]);
        cmd.args(video_args(&enc, &Rate::Quality(27), settings.cpu_threads));
        cmd.args(["-g", "30"]);
    }
    cmd.args(["-c:a", "aac", "-b:a", "128k", "-ac", "2", "-movflags", "+faststart", "-f", "mp4"]);
    cmd.arg(&tmp);

    let dur = meta.duration.max(0.001);
    let res = run_progress(cmd, &token, |t| {
        let _ = app.emit(
            "proxy-progress",
            ProxyProgress { path: path.clone(), percent: (t / dur * 100.0).clamp(0.0, 100.0) },
        );
    })
    .await;
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, &out)?;
    state.allow(&out);
    Ok(out.to_string_lossy().into_owned())
}

/// Удалить прокси старше 3 дней и временные файлы прерванных экспортов.
pub async fn cleanup_old(app: &AppHandle) {
    let max_age = Duration::from_secs(3 * 24 * 3600);
    let now = SystemTime::now();
    for dir in [proxy_dir(app).ok(), tmp_root(app).ok()].into_iter().flatten() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| now.duration_since(t).unwrap_or_default() > max_age)
                .unwrap_or(false);
            let is_tmp_dir = dir.ends_with("tmp");
            if old || is_tmp_dir {
                let p = e.path();
                let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
        }
    }
}

pub fn tmp_root(app: &AppHandle) -> AppResult<PathBuf> {
    let d = app
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::msg(e.to_string()))?
        .join("tmp");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn dir_size(p: &Path) -> u64 {
    std::fs::read_dir(p)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_size(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

#[tauri::command]
pub fn preview_cache_size(app: AppHandle) -> AppResult<u64> {
    Ok(dir_size(&proxy_dir(&app)?))
}

#[tauri::command]
pub fn clear_preview_cache(app: AppHandle) -> AppResult<()> {
    let d = proxy_dir(&app)?;
    for e in std::fs::read_dir(&d)?.flatten() {
        let _ = std::fs::remove_file(e.path());
    }
    Ok(())
}
