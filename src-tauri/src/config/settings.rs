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
    /// Фоновый захват и буфер повтора.
    pub capture: CaptureSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: String::new(),
            priority: Priority::BelowNormal,
            cpu_threads: 0,
            encoders: Vec::new(),
            ui: serde_json::Value::Null,
            capture: CaptureSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptureSettings {
    /// monitor | window | auto (следовать за полноэкранной игрой)
    pub source: String,
    /// exe, которые не считать играми в режиме auto.
    pub game_exclude: Vec<String>,
    /// Индекс выхода DXGI (монитора).
    pub monitor: u32,
    /// Окно запоминается по exe (HWND меняется между запусками игры).
    pub window_exe: String,
    pub window_title: String,
    pub fps: u32,
    /// Высота кадра записи, 0 = исходная.
    pub height: u32,
    /// h264 | hevc
    pub codec: String,
    /// auto | nvenc | amf | qsv | cpu
    pub encoder: String,
    pub bitrate_mbps: f64,
    /// Длина буфера повтора, секунд.
    pub buffer_secs: u32,
    pub system_audio: bool,
    /// Только звук выбранного окна (Process Loopback) вместо всего системного.
    pub app_audio_only: bool,
    pub mic: bool,
    /// ID устройства, "" = микрофон по умолчанию.
    pub mic_device: String,
    /// Сдвиг звука относительно видео (мс, плюс = позже).
    pub audio_offset_ms: i32,
    pub hotkey_save: String,
    pub hotkey_record: String,
    /// Включать буфер повтора при запуске приложения.
    pub auto_start: bool,
    /// Звуковой сигнал при сохранении клипа.
    pub sound: bool,
    /// Закрытие окна сворачивает в трей (WebView2 уничтожается, захват продолжается).
    pub close_to_tray: bool,
    /// Прямой захват AMD (vsrc_amf, zero-copy). Экспериментально: при резкой остановке во время
    /// игры был замечен сбой драйвера, поэтому по умолчанию используется Windows.Graphics.Capture.
    pub amf_direct: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            source: "monitor".into(),
            game_exclude: Vec::new(),
            monitor: 0,
            window_exe: String::new(),
            window_title: String::new(),
            fps: 60,
            height: 0,
            codec: "h264".into(),
            encoder: "auto".into(),
            bitrate_mbps: 20.0,
            buffer_secs: 60,
            system_audio: true,
            app_audio_only: false,
            mic: false,
            mic_device: String::new(),
            audio_offset_ms: 0,
            hotkey_save: "Alt+F10".into(),
            hotkey_record: "Alt+F9".into(),
            auto_start: false,
            sound: true,
            close_to_tray: true,
            amf_direct: false,
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
