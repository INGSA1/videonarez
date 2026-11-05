//! Настройки приложения в JSON (папка конфигурации пользователя).

use crate::config::gpu::EncoderInfo;
use crate::error::AppResult;
use crate::ffmpeg::Priority;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Папка для готовых клипов.
    pub output_dir: String,
    /// Приоритет процессов FFmpeg при экспорте.
    pub priority: Priority,
    /// Лимит потоков CPU-кодировщика (0 = авто).
    pub cpu_threads: u32,
    /// Кэш результатов автоопределения кодировщиков.
    pub encoders: Vec<EncoderInfo>,
    /// Состояние интерфейса (последние параметры экспорта и т.п.) — принадлежит фронтенду.
    pub ui: serde_json::Value,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: String::new(),
            priority: Priority::BelowNormal,
            cpu_threads: 0,
            encoders: Vec::new(),
            ui: serde_json::Value::Null,
        }
    }
}

pub fn init(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("settings.json");
    let mut s: Settings = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if s.output_dir.is_empty() {
        let base = app
            .path()
            .video_dir()
            .or_else(|_| app.path().home_dir())?;
        s.output_dir = base.join("Videonarez").to_string_lossy().into_owned();
    }
    let st = app.state::<AppState>();
    *st.settings.write().unwrap() = s;
    *st.settings_path.lock().unwrap() = Some(path);
    Ok(())
}

pub fn persist(state: &AppState) -> AppResult<()> {
    let path = state.settings_path.lock().unwrap().clone();
    if let Some(p) = path {
        let s = state.settings();
        std::fs::write(&p, serde_json::to_vec_pretty(&s)?)?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> AppResult<()> {
    {
        let mut g = state.settings.write().unwrap();
        let encoders = std::mem::take(&mut g.encoders);
        *g = settings;
        if g.encoders.is_empty() {
            g.encoders = encoders;
        }
    }
    persist(&state)
}
