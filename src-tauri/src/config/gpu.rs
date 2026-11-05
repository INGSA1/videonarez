//! Автоопределение аппаратных кодировщиков.
//!
//! Вместо угадывания по Vendor ID каждый GPU-кодировщик проверяется пробным
//! кодированием 5 кадров: так отсекаются энкодеры, которые есть в сборке FFmpeg,
//! но не поддерживаются видеокартой или драйвером. Результат кэшируется в настройках.

use crate::config::settings::persist;
use crate::error::AppResult;
use crate::ffmpeg::{ffmpeg_cmd, output, Priority};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderInfo {
    pub name: String,
    /// nvenc | amf | qsv | cpu
    pub vendor: String,
    /// h264 | hevc | av1
    pub codec: String,
    pub hardware: bool,
}

const CANDIDATES: &[(&str, &str, &str, bool)] = &[
    ("h264_nvenc", "nvenc", "h264", true),
    ("hevc_nvenc", "nvenc", "hevc", true),
    ("av1_nvenc", "nvenc", "av1", true),
    ("h264_amf", "amf", "h264", true),
    ("hevc_amf", "amf", "hevc", true),
    ("av1_amf", "amf", "av1", true),
    ("h264_qsv", "qsv", "h264", true),
    ("hevc_qsv", "qsv", "hevc", true),
    ("av1_qsv", "qsv", "av1", true),
    ("libx264", "cpu", "h264", false),
    ("libx265", "cpu", "hevc", false),
    ("libsvtav1", "cpu", "av1", false),
    ("libaom-av1", "cpu", "av1", false),
];

const VENDOR_ORDER: &[&str] = &["nvenc", "amf", "qsv", "cpu"];

pub async fn detect() -> AppResult<Vec<EncoderInfo>> {
    let mut cmd = ffmpeg_cmd(Priority::Normal, false)?;
    cmd.arg("-encoders");
    let list = String::from_utf8_lossy(&output(cmd, None).await?).into_owned();
    let present = |n: &str| list.lines().any(|l| l.split_whitespace().nth(1) == Some(n));

    let mut found = Vec::new();
    let mut cpu_av1 = false;
    for &(name, vendor, codec, hw) in CANDIDATES {
        if !present(name) {
            continue;
        }
        if hw {
            if !test_encoder(name).await {
                continue;
            }
        } else if codec == "av1" {
            if cpu_av1 {
                continue;
            }
            cpu_av1 = true;
        }
        found.push(EncoderInfo {
            name: name.into(),
            vendor: vendor.into(),
            codec: codec.into(),
            hardware: hw,
        });
    }
    Ok(found)
}

async fn test_encoder(name: &str) -> bool {
    let Ok(mut cmd) = ffmpeg_cmd(Priority::Normal, false) else {
        return false;
    };
    cmd.args([
        "-f", "lavfi", "-i", "color=c=black:s=1280x720:r=30",
        "-frames:v", "5", "-pix_fmt", "nv12", "-c:v", name, "-f", "null", "-",
    ]);
    matches!(
        tokio::time::timeout(Duration::from_secs(12), output(cmd, None)).await,
        Ok(Ok(_))
    )
}

/// Выбрать кодировщик: vendor = "auto" → первый доступный из NVENC → AMF → QSV → CPU.
pub fn pick(list: &[EncoderInfo], vendor: &str, codec: &str) -> Option<EncoderInfo> {
    let by = |v: &str| {
        list.iter()
            .find(|e| e.vendor == v && e.codec == codec)
            .cloned()
    };
    let found = if vendor == "auto" {
        VENDOR_ORDER.iter().find_map(|v| by(v))
    } else {
        by(vendor)
    };
    found.or_else(|| {
        // Определение ещё не выполнялось — используем программный кодировщик.
        if list.is_empty() {
            let name = match codec {
                "hevc" => "libx265",
                "av1" => "libaom-av1",
                _ => "libx264",
            };
            Some(EncoderInfo {
                name: name.into(),
                vendor: "cpu".into(),
                codec: codec.into(),
                hardware: false,
            })
        } else {
            None
        }
    })
}

pub async fn ensure_detected(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.settings().encoders.is_empty() {
        return;
    }
    match detect().await {
        Ok(list) => {
            state.settings.write().unwrap().encoders = list;
            let _ = persist(&state);
        }
        Err(e) => eprintln!("encoder detection failed: {e}"),
    }
}

#[tauri::command]
pub async fn detect_encoders(state: State<'_, AppState>, force: bool) -> AppResult<Vec<EncoderInfo>> {
    let cached = state.settings().encoders;
    if !force && !cached.is_empty() {
        return Ok(cached);
    }
    let list = detect().await?;
    state.settings.write().unwrap().encoders = list.clone();
    persist(&state)?;
    Ok(list)
}
