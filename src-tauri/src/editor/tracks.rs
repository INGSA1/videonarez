//! Звуковые дорожки для просмотра в редакторе.
//!
//! WebView2 воспроизводит из видеофайла только первую звуковую дорожку, поэтому для файлов
//! с несколькими дорожками каждая выносится в отдельный лёгкий .m4a (копированием, если это AAC,
//! иначе быстрым перекодированием). Редактор проигрывает их синхронно с видео через Web Audio,
//! и у каждой дорожки своя громкость и выключатель.

use crate::editor::probe;
use crate::error::{AppError, AppResult};
use crate::ffmpeg::{ffmpeg_cmd, output, Priority};
use crate::state::AppState;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

fn key(path: &str) -> String {
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

/// Пути к файлам дорожек (по индексу дорожки). Готовые берутся из кэша.
#[tauri::command]
pub async fn extract_audio_tracks(app: AppHandle, state: State<'_, AppState>, path: String) -> AppResult<Vec<String>> {
    if !state.is_allowed(std::path::Path::new(&path)) {
        return Err(AppError::msg("Файл не открыт в редакторе"));
    }
    let meta = probe::probe(&path).await?;
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::msg(e.to_string()))?
        .join("tracks");
    std::fs::create_dir_all(&dir)?;
    let token = state.analysis();
    let k = key(&path);
    let mut out = Vec::new();
    for t in &meta.audio_tracks {
        let file: PathBuf = dir.join(format!("{k}_{}.m4a", t.index));
        if !file.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            let tmp = file.with_extension("part");
            let mut cmd = ffmpeg_cmd(Priority::BelowNormal, false)?;
            cmd.arg("-i").arg(&path);
            cmd.args(["-map", &format!("0:a:{}", t.index), "-vn", "-sn", "-dn"]);
            if matches!(t.codec.as_str(), "aac" | "mp3" | "alac") {
                cmd.args(["-c:a", "copy"]);
            } else {
                cmd.args(["-c:a", "aac", "-b:a", "160k"]);
            }
            cmd.args(["-movflags", "+faststart", "-f", "mp4"]).arg(&tmp);
            if let Err(e) = output(cmd, Some(&token)).await {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            std::fs::rename(&tmp, &file)?;
        }
        state.allow(&file);
        out.push(file.to_string_lossy().into_owned());
    }
    Ok(out)
}
