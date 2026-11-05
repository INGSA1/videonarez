//! Метаданные видео через ffprobe (только заголовки контейнера, без декодирования).

use crate::error::{AppError, AppResult};
use crate::ffmpeg::{ffprobe_cmd, output};
use crate::state::AppState;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    /// Индекс среди аудиодорожек (0:a:N).
    pub index: usize,
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub title: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMeta {
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub duration: f64,
    pub start_time: f64,
    pub format_name: String,
    pub bit_rate: Option<u64>,
    pub has_video: bool,
    pub width: u32,
    pub height: u32,
    /// Размеры с учётом поворота (как видит плеер и фильтры FFmpeg).
    pub display_width: u32,
    pub display_height: u32,
    pub rotation: i32,
    pub fps: f64,
    pub video_codec: String,
    pub profile: Option<String>,
    pub pix_fmt: String,
    pub audio_tracks: Vec<AudioTrack>,
    /// WebView2 вряд ли сможет воспроизвести файл напрямую — нужен прокси.
    pub needs_proxy: bool,
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn rate(v: &Value) -> Option<f64> {
    let s = v.as_str()?;
    let (a, b) = s.split_once('/').unwrap_or((s, "1"));
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    (b > 0.0 && a > 0.0).then(|| a / b)
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

pub async fn probe(path: &str) -> AppResult<VideoMeta> {
    let p = Path::new(path);
    let md = std::fs::metadata(p).map_err(|e| AppError::msg(format!("Не удалось открыть файл: {e}")))?;

    let mut cmd = ffprobe_cmd()?;
    cmd.args(["-print_format", "json", "-show_format", "-show_streams"]).arg(path);
    let v: Value = serde_json::from_slice(&output(cmd, None).await?)?;
    let format = &v["format"];
    let streams = v["streams"].as_array().cloned().unwrap_or_default();

    let video = streams.iter().find(|st| {
        st["codec_type"] == "video" && st["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0
    });
    let audio_tracks: Vec<AudioTrack> = streams
        .iter()
        .filter(|st| st["codec_type"] == "audio")
        .enumerate()
        .map(|(i, st)| AudioTrack {
            index: i,
            codec: s(&st["codec_name"]),
            channels: st["channels"].as_u64().unwrap_or(2) as u32,
            sample_rate: num(&st["sample_rate"]).unwrap_or(48000.0) as u32,
            // MP4 хранит имя дорожки в handler_name; стандартные "SoundHandler" и т.п. пропускаем.
            title: st["tags"]["title"]
                .as_str()
                .or_else(|| st["tags"]["handler_name"].as_str())
                .filter(|t| !t.is_empty() && !t.ends_with("Handler") && !t.starts_with("ISO Media"))
                .map(String::from),
            language: st["tags"]["language"].as_str().map(String::from),
        })
        .collect();

    let duration = num(&format["duration"])
        .or_else(|| video.and_then(|st| num(&st["duration"])))
        .unwrap_or(0.0);
    if duration <= 0.0 {
        return Err(AppError::msg("Не удалось определить длительность видео"));
    }

    let (width, height, fps, codec, profile, pix_fmt, rotation) = match video {
        Some(st) => {
            let rot = st["side_data_list"]
                .as_array()
                .and_then(|l| l.iter().find_map(|d| num(&d["rotation"])))
                .or_else(|| num(&st["tags"]["rotate"]))
                .unwrap_or(0.0) as i32;
            (
                st["width"].as_u64().unwrap_or(0) as u32,
                st["height"].as_u64().unwrap_or(0) as u32,
                rate(&st["avg_frame_rate"])
                    .or_else(|| rate(&st["r_frame_rate"]))
                    .unwrap_or(30.0),
                s(&st["codec_name"]),
                st["profile"].as_str().map(String::from),
                s(&st["pix_fmt"]),
                ((rot % 360) + 360) % 360,
            )
        }
        None => (0, 0, 30.0, String::new(), None, String::new(), 0),
    };
    let (display_width, display_height) = if rotation == 90 || rotation == 270 {
        (height, width)
    } else {
        (width, height)
    };

    let format_name = s(&format["format_name"]);
    let needs_proxy = video.is_some() && !browser_playable(&format_name, &codec, &pix_fmt, &audio_tracks);

    Ok(VideoMeta {
        path: path.to_string(),
        file_name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size_bytes: md.len(),
        duration,
        start_time: num(&format["start_time"]).unwrap_or(0.0),
        format_name,
        bit_rate: num(&format["bit_rate"]).map(|b| b as u64),
        has_video: video.is_some(),
        width,
        height,
        display_width,
        display_height,
        rotation,
        fps,
        video_codec: codec,
        profile,
        pix_fmt,
        audio_tracks,
        needs_proxy,
    })
}

/// Грубая оценка: что WebView2 (Chromium) играет нативно с аппаратным декодированием.
fn browser_playable(format: &str, vcodec: &str, pix_fmt: &str, audio: &[AudioTrack]) -> bool {
    let is_mp4 = format.contains("mp4") || format.contains("mov");
    let is_webm = format.contains("matroska") || format.contains("webm");
    let video_ok = match vcodec {
        "h264" => matches!(pix_fmt, "yuv420p" | "yuvj420p"),
        "hevc" | "av1" | "vp9" => matches!(pix_fmt, "yuv420p" | "yuvj420p" | "yuv420p10le"),
        "vp8" => true,
        _ => false,
    };
    let container_ok = if is_mp4 {
        matches!(vcodec, "h264" | "hevc" | "av1" | "vp9")
    } else if is_webm {
        // Chromium принимает Matroska только как WebM-кодеки.
        matches!(vcodec, "vp8" | "vp9" | "av1")
    } else {
        false
    };
    let audio_ok = audio.first().map_or(true, |a| {
        matches!(a.codec.as_str(), "aac" | "mp3" | "opus" | "flac" | "vorbis")
    });
    video_ok && container_ok && audio_ok
}

#[tauri::command]
pub async fn probe_video_metadata(state: State<'_, AppState>, path: String) -> AppResult<VideoMeta> {
    state.new_analysis(); // отменить анализ предыдущего файла
    let meta = probe(&path).await?;
    state.allow(Path::new(&path));
    Ok(meta)
}
