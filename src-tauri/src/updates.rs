//! Автообновление: проверка latest.json на сервере, загрузка, проверка подписи и установка.
//!
//! Каждый релиз подписан приватным ключом (minisign). Плагин проверяет подпись публичным ключом
//! из tauri.conf.json до запуска установщика, поэтому подменённый файл установлен не будет.

use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct Pending(pub Mutex<Option<Update>>);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

async fn check_inner(app: &AppHandle) -> AppResult<Option<UpdateInfo>> {
    let updater = app.updater().map_err(|e| AppError::msg(format!("Обновления недоступны: {e}")))?;
    let found = updater
        .check()
        .await
        .map_err(|e| AppError::msg(format!("Не удалось проверить обновления: {e}")))?;
    let info = found.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current: u.current_version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.to_string()),
    });
    *app.state::<Pending>().0.lock().unwrap() = found;
    Ok(info)
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> AppResult<Option<UpdateInfo>> {
    check_inner(&app).await
}

/// Скачать (с проверкой подписи) и установить найденное обновление, затем перезапуститься.
/// `dry_run` — только скачать и проверить подпись, ничего не устанавливая (для проверки канала).
#[tauri::command]
pub async fn install_update(app: AppHandle, dry_run: Option<bool>) -> AppResult<u64> {
    let update = app
        .state::<Pending>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| AppError::msg("Сначала проверьте наличие обновления"))?;

    let mut downloaded = 0u64;
    let a = app.clone();
    let bytes = update
        .download(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = a.emit("update-progress", Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| AppError::msg(format!("Не удалось скачать обновление: {e}")))?;
    let size = bytes.len() as u64;
    // В отладочной сборке переменная VIDEONAREZ_UPDATE_DRY_RUN включает режим «только проверить»:
    // канал и подпись проверяются целиком, но установщик не запускается.
    let dry = dry_run.unwrap_or(false)
        || (cfg!(debug_assertions) && std::env::var_os("VIDEONAREZ_UPDATE_DRY_RUN").is_some());
    if dry {
        return Ok(size);
    }

    // Мягко останавливаем захват до запуска установщика (он закроет приложение).
    crate::capture::recorder::stop(&app);
    update
        .install(bytes)
        .map_err(|e| AppError::msg(format!("Не удалось установить обновление: {e}")))?;
    app.restart();
}

/// Фоновая проверка через полминуты после запуска и затем раз в 6 часов.
pub fn spawn_background_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        loop {
            if let Ok(Some(info)) = check_inner(&app).await {
                let _ = app.emit("update-available", info);
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });
}
