//! Миниатюры для таймлайна: только по ключевым кадрам, без декодирования остального видео.
//!
//! Каждая миниатюра — отдельный быстрый seek (-ss перед -i, -noaccurate_seek, -skip_frame nokey),
//! максимум 3 процесса параллельно, вывод JPEG прямо в pipe (без временных файлов).
//! Готовые кадры уходят во фронтенд событиями по мере готовности.

use crate::error::{AppError, AppResult};
use crate::ffmpeg::{ffmpeg_cmd, output, ts, Priority};
use crate::state::AppState;
use base64::Engine;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ThumbEvent {
    path: String,
    index: usize,
    time: f64,
    data: String,
}

#[tauri::command]
pub async fn extract_thumbnails(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    count: u32,
    duration: f64,
    height: u32,
) -> AppResult<()> {
    let token = state.analysis();
    let count = count.clamp(1, 60) as usize;
    let height = height.clamp(32, 240);
    let sem = Arc::new(Semaphore::new(3));
    let mut set = JoinSet::new();

    for i in 0..count {
        let t = duration * (i as f64 + 0.5) / count as f64;
        let (app, path, token, sem) = (app.clone(), path.clone(), token.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok()?;
            if token.is_cancelled() {
                return None;
            }
            let mut cmd = ffmpeg_cmd(Priority::BelowNormal, false).ok()?;
            cmd.args(["-noaccurate_seek", "-skip_frame", "nokey", "-ss", &ts(t), "-i", &path]);
            cmd.args([
                "-map", "0:v:0", "-an", "-sn", "-frames:v", "1",
                "-vf", &format!("scale=-2:{height}:flags=fast_bilinear,format=yuvj420p"),
                "-c:v", "mjpeg", "-q:v", "6", "-f", "image2pipe", "pipe:1",
            ]);
            let jpg = output(cmd, Some(&token)).await.ok()?;
            if jpg.is_empty() {
                return None;
            }
            let data = format!(
                "data:image/jpeg;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&jpg)
            );
            let _ = app.emit("thumbnail", ThumbEvent { path, index: i, time: t, data });
            Some(())
        });
    }
    while set.join_next().await.is_some() {}
    if token.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    Ok(())
}
