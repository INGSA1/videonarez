//! Галерея клипов: последние видео из папки клипов и кадр-превью для каждого.
//!
//! Превью — один ключевой кадр 320 px (декодируется только он), кэшируется на диске
//! по пути + размеру + времени изменения, поэтому повторное открытие галереи бесплатно.

use crate::error::{AppError, AppResult};
use crate::ffmpeg::{ffmpeg_cmd, ffprobe_cmd, output, ts, Priority};
use crate::state::AppState;
use base64::Engine;
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Manager, State};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClipFile {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    /// Время изменения, мс от эпохи Unix.
    pub modified: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipPreview {
    pub thumb: Option<String>,
    pub duration: f64,
}

const EXT: &[&str] = &["mp4", "mkv", "mov", "webm"];

#[tauri::command]
pub fn list_clips(state: State<'_, AppState>, limit: usize) -> AppResult<Vec<ClipFile>> {
    let dir = PathBuf::from(state.settings().output_dir);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut v: Vec<ClipFile> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_string_lossy().to_lowercase();
            let name = p.file_name()?.to_string_lossy().into_owned();
            // скрытые/временные файлы записи не показываем
            if !EXT.contains(&ext.as_str()) || name.starts_with('.') {
                return None;
            }
            let md = e.metadata().ok()?;
            let modified = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
            Some(ClipFile { path: p.to_string_lossy().into_owned(), name, size_bytes: md.len(), modified })
        })
        .collect();
    v.sort_by(|a, b| b.modified.cmp(&a.modified));
    v.truncate(limit.clamp(1, 200));
    Ok(v)
}

fn cache_path(app: &AppHandle, path: &Path) -> AppResult<PathBuf> {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        m.len().hash(&mut h);
        if let Ok(t) = m.modified() {
            t.hash(&mut h);
        }
    }
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::msg(e.to_string()))?
        .join("thumbs");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{:016x}.jpg", h.finish())))
}

async fn duration(path: &str) -> f64 {
    let Ok(mut cmd) = ffprobe_cmd() else { return 0.0 };
    cmd.args(["-show_entries", "format=duration", "-of", "csv=p=0"]).arg(path);
    output(cmd, None)
        .await
        .ok()
        .and_then(|b| String::from_utf8_lossy(&b).trim().parse().ok())
        .unwrap_or(0.0)
}

#[tauri::command]
pub async fn clip_preview(app: AppHandle, path: String) -> AppResult<ClipPreview> {
    let src = PathBuf::from(&path);
    if !src.is_file() {
        return Err(AppError::msg("Файл не найден"));
    }
    let dur = duration(&path).await;
    let cache = cache_path(&app, &src)?;
    if !cache.is_file() {
        // Кадр примерно с трети клипа — обычно уже «самое интересное», но не самый конец.
        let at = if dur > 0.0 { (dur * 0.33).min(dur - 0.1).max(0.0) } else { 0.0 };
        let mut cmd = ffmpeg_cmd(Priority::BelowNormal, false)?;
        cmd.args(["-noaccurate_seek", "-skip_frame", "nokey", "-ss", &ts(at), "-i", &path]);
        cmd.args([
            "-map", "0:v:0", "-an", "-frames:v", "1",
            "-vf", "scale=320:-2:flags=bicubic,format=yuvj420p",
            "-q:v", "4", "-f", "image2",
        ]);
        cmd.arg(&cache);
        if output(cmd, None).await.is_err() {
            return Ok(ClipPreview { thumb: None, duration: dur });
        }
    }
    let thumb = std::fs::read(&cache).ok().map(|b| {
        format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b))
    });
    Ok(ClipPreview { thumb, duration: dur })
}

/// Открыть папку клипов в проводнике.
#[tauri::command]
pub fn open_clips_folder(state: State<'_, AppState>) -> AppResult<()> {
    let dir = state.settings().output_dir;
    std::fs::create_dir_all(&dir)?;
    std::process::Command::new("explorer").arg(&dir).spawn()?;
    Ok(())
}
