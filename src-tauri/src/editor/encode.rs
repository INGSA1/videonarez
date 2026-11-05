//! Аргументы видеокодировщиков (NVENC / AMF / QSV / CPU) для режимов качества и битрейта.

use crate::config::gpu::EncoderInfo;

pub enum Rate {
    /// Постоянное качество, шкала как у CRF x264 (0–51, меньше = лучше).
    Quality(u32),
    /// Средний битрейт с ограничением пика (кбит/с).
    Bitrate { kbps: u32, max_kbps: u32 },
}

fn push(a: &mut Vec<String>, xs: &[&str]) {
    a.extend(xs.iter().map(|s| s.to_string()));
}

/// Шкала 0–51 → 0–255 (AV1 у AMF/QSV).
fn q255(q: u32) -> u32 {
    (q * 255 / 51).min(255)
}

/// Шкала 0–51 → 0–63 (CRF у SVT-AV1 / libaom).
fn q63(q: u32) -> u32 {
    (q * 63 / 51).min(63)
}

pub fn video_args(enc: &EncoderInfo, rate: &Rate, threads: u32) -> Vec<String> {
    let mut a = vec!["-c:v".to_string(), enc.name.clone()];
    let codec = enc.codec.as_str();
    match (enc.vendor.as_str(), rate) {
        ("amf", Rate::Quality(q)) => {
            let q = if codec == "av1" { q255(*q) } else { *q }.to_string();
            push(&mut a, &["-quality", "quality", "-rc", "cqp", "-qp_i", &q, "-qp_p", &q]);
            if codec == "h264" {
                push(&mut a, &["-qp_b", &q]);
            }
        }
        ("amf", Rate::Bitrate { kbps, max_kbps }) => {
            push(&mut a, &[
                "-quality", "quality", "-rc", "vbr_peak",
                "-b:v", &format!("{kbps}k"), "-maxrate", &format!("{max_kbps}k"),
                "-bufsize", &format!("{}k", kbps * 2),
            ]);
        }
        ("nvenc", Rate::Quality(q)) => {
            push(&mut a, &["-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", &q.to_string(), "-b:v", "0"]);
        }
        ("nvenc", Rate::Bitrate { kbps, max_kbps }) => {
            push(&mut a, &[
                "-preset", "p5", "-tune", "hq", "-rc", "vbr",
                "-b:v", &format!("{kbps}k"), "-maxrate", &format!("{max_kbps}k"),
                "-bufsize", &format!("{}k", kbps * 2),
            ]);
        }
        ("qsv", Rate::Quality(q)) => {
            let q = if codec == "av1" { q255(*q) } else { *q }.to_string();
            push(&mut a, &["-preset", "medium", "-global_quality", &q]);
        }
        ("qsv", Rate::Bitrate { kbps, max_kbps }) => {
            push(&mut a, &[
                "-preset", "medium", "-b:v", &format!("{kbps}k"),
                "-maxrate", &format!("{max_kbps}k"), "-bufsize", &format!("{}k", kbps * 2),
            ]);
        }
        // CPU-кодировщики
        (_, rate) => {
            match enc.name.as_str() {
                "libx264" => push(&mut a, &["-preset", "veryfast"]),
                "libx265" => {
                    let mut params = "log-level=error".to_string();
                    if threads > 0 {
                        params.push_str(&format!(":pools={threads}"));
                    }
                    push(&mut a, &["-preset", "fast", "-x265-params", &params]);
                }
                "libsvtav1" => push(&mut a, &["-preset", "8"]),
                "libaom-av1" => push(&mut a, &["-cpu-used", "6", "-row-mt", "1"]),
                _ => {}
            }
            match rate {
                Rate::Quality(q) => {
                    let crf = if codec == "av1" { q63(*q) } else { *q };
                    push(&mut a, &["-crf", &crf.to_string()]);
                    if enc.name == "libaom-av1" {
                        push(&mut a, &["-b:v", "0"]);
                    }
                }
                Rate::Bitrate { kbps, max_kbps } => push(&mut a, &[
                    "-b:v", &format!("{kbps}k"), "-maxrate", &format!("{max_kbps}k"),
                    "-bufsize", &format!("{}k", kbps * 2),
                ]),
            }
            if threads > 0 && enc.name != "libx265" {
                push(&mut a, &["-threads", &threads.to_string()]);
            }
        }
    }
    a
}

/// Формат пикселей на входе кодировщика: GPU-энкодеры лучше всего принимают NV12.
pub fn pix_fmt(enc: &EncoderInfo) -> &'static str {
    if enc.hardware {
        "nv12"
    } else {
        "yuv420p"
    }
}

/// Аргументы контейнера на выходе.
pub fn container_args(container: &str, hevc: bool) -> Vec<String> {
    let mut a = Vec::new();
    if container == "mkv" {
        push(&mut a, &["-f", "matroska"]);
    } else {
        if hevc {
            push(&mut a, &["-tag:v", "hvc1"]); // совместимость с плеерами Apple/браузерами
        }
        push(&mut a, &["-movflags", "+faststart", "-f", "mp4"]);
    }
    a
}
