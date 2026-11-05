//! Звуковая волна и автоматические маркеры хайлайтов по пикам громкости.
//!
//! Аудио декодируется в моно 8 кГц и читается потоком из pipe: в памяти держатся только
//! накопленные RMS-значения корзин (несколько тысяч f32), а не весь PCM.

use crate::error::{AppError, AppResult};
use crate::ffmpeg::{ffmpeg_cmd, tail, Priority};
use crate::state::AppState;
use serde::Serialize;
use std::process::Stdio;
use tauri::State;
use tokio::io::AsyncReadExt;

const SR: u32 = 8000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub time: f64,
    /// 0..1 — относительная сила пика.
    pub score: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waveform {
    pub path: String,
    pub track: usize,
    /// Нормированная громкость 0..1 (шкала -60..0 дБ) на корзину.
    pub peaks: Vec<f32>,
    pub bucket_sec: f64,
    pub markers: Vec<Marker>,
}

#[tauri::command]
pub async fn extract_waveform(
    state: State<'_, AppState>,
    path: String,
    track: usize,
    duration: f64,
) -> AppResult<Waveform> {
    let token = state.analysis();
    let points = ((duration * 10.0) as usize).clamp(400, 8000);
    let total_samples = (duration * SR as f64).max(1.0);
    let per_bucket = (total_samples / points as f64).ceil().max(1.0) as usize;

    let mut cmd = ffmpeg_cmd(Priority::BelowNormal, false)?;
    cmd.arg("-i").arg(&path);
    cmd.args([
        "-map", &format!("0:a:{track}"), "-vn", "-sn", "-dn",
        "-ac", "1", "-ar", &SR.to_string(), "-f", "s16le", "pipe:1",
    ]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let err_task = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b).await;
        b
    });

    let mut rms: Vec<f32> = Vec::with_capacity(points + 8);
    let mut acc = 0f64;
    let mut cnt = 0usize;
    let mut buf = vec![0u8; 64 * 1024];
    let mut carry: Option<u8> = None;

    loop {
        let n = tokio::select! {
            r = stdout.read(&mut buf) => r?,
            _ = token.cancelled() => { let _ = child.kill().await; return Err(AppError::Cancelled); }
        };
        if n == 0 {
            break;
        }
        let mut i = 0;
        let sample = |lo: u8, hi: u8, acc: &mut f64, cnt: &mut usize, rms: &mut Vec<f32>| {
            let v = i16::from_le_bytes([lo, hi]) as f64;
            *acc += v * v;
            *cnt += 1;
            if *cnt == per_bucket {
                rms.push((*acc / *cnt as f64).sqrt() as f32);
                *acc = 0.0;
                *cnt = 0;
            }
        };
        if let Some(lo) = carry.take() {
            sample(lo, buf[0], &mut acc, &mut cnt, &mut rms);
            i = 1;
        }
        while i + 1 < n {
            sample(buf[i], buf[i + 1], &mut acc, &mut cnt, &mut rms);
            i += 2;
        }
        if i < n {
            carry = Some(buf[i]);
        }
    }
    if cnt > 0 {
        rms.push((acc / cnt as f64).sqrt() as f32);
    }
    let status = child.wait().await?;
    let err = err_task.await.unwrap_or_default();
    if !status.success() {
        return Err(AppError::Msg(tail(&err)));
    }

    let bucket_sec = per_bucket as f64 / SR as f64;
    // дБ относительно полной шкалы
    let db: Vec<f32> = rms
        .iter()
        .map(|&r| (20.0 * (r.max(1.0) / 32768.0).log10()).clamp(-60.0, 0.0))
        .collect();
    let peaks: Vec<f32> = db.iter().map(|d| (d + 60.0) / 60.0).collect();
    let markers = detect_markers(&db, bucket_sec);

    Ok(Waveform { path, track, peaks, bucket_sec, markers })
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// Громкие всплески (выстрелы, крики, взрывы): локальные максимумы сглаженной громкости,
/// заметно выше медианного фона, не чаще одного на 4 секунды, максимум 40.
fn detect_markers(db: &[f32], bucket_sec: f64) -> Vec<Marker> {
    if db.len() < 8 {
        return Vec::new();
    }
    let win = ((0.4 / bucket_sec).round() as usize).max(1);
    let smooth: Vec<f32> = (0..db.len())
        .map(|i| {
            let a = i.saturating_sub(win / 2);
            let b = (i + win / 2 + 1).min(db.len());
            db[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect();

    let mut tmp = smooth.clone();
    let med = median(&mut tmp);
    let mut dev: Vec<f32> = smooth.iter().map(|v| (v - med).abs()).collect();
    let mad = median(&mut dev) * 1.4826;
    let threshold = med + (3.0 * mad).max(6.0);
    if med <= -59.0 {
        return Vec::new(); // тишина
    }

    let radius = ((2.0 / bucket_sec).round() as usize).max(1);
    let mut cands: Vec<(usize, f32)> = Vec::new();
    for i in 0..smooth.len() {
        let v = smooth[i];
        if v < threshold {
            continue;
        }
        let a = i.saturating_sub(radius);
        let b = (i + radius + 1).min(smooth.len());
        if smooth[a..b].iter().all(|&x| x <= v) {
            cands.push((i, v - med));
        }
    }
    cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let min_gap = 4.0;
    let mut picked: Vec<(f64, f32)> = Vec::new();
    for (i, score) in cands {
        let t = (i as f64 + 0.5) * bucket_sec;
        if picked.iter().all(|(pt, _)| (pt - t).abs() >= min_gap) {
            picked.push((t, score));
            if picked.len() >= 40 {
                break;
            }
        }
    }
    let max = picked.iter().map(|p| p.1).fold(0.0f32, f32::max).max(0.001);
    picked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    picked
        .into_iter()
        .map(|(time, s)| Marker { time, score: (s / max).clamp(0.0, 1.0) })
        .collect()
}
