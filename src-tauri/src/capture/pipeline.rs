//! Командная строка FFmpeg для захвата: источник → (масштаб на GPU) → аппаратный энкодер → MPEG-TS в stdout.
//!
//! Кандидаты перебираются по порядку («лестница»): сначала zero-copy путь для текущей видеокарты,
//! где кадр ни разу не покидает видеопамять, затем универсальный путь через Windows.Graphics.Capture
//! с копированием кадра в ОЗУ, и в конце — DXGI Desktop Duplication.

use super::audio::RATE;
use super::ring::{PMT_PID, VIDEO_PID};
use super::sources::{MonitorInfo, WindowInfo};
use crate::config::gpu::EncoderInfo;
use crate::config::settings::CaptureSettings;

#[derive(Debug, Clone)]
pub enum Target {
    Monitor(MonitorInfo),
    Window(WindowInfo),
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Target::Monitor(m) => format!("Монитор {} ({}×{})", m.index + 1, m.width, m.height),
            Target::Window(w) => format!("Окно: {}", if w.title.is_empty() { &w.exe } else { &w.title }),
        }
    }

    /// Префикс имени файла клипа.
    pub fn file_prefix(&self) -> String {
        match self {
            Target::Monitor(_) => "Replay".into(),
            Target::Window(w) => {
                let stem = w.exe.trim_end_matches(".exe").trim_end_matches(".EXE");
                if stem.is_empty() { "Replay".into() } else { stem.to_string() }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub label: String,
    pub zero_copy: bool,
    pub graph: String,
}

fn even(v: u32) -> u32 {
    v - v % 2
}

fn out_size(w: u32, h: u32, target_h: u32) -> (u32, u32) {
    if target_h == 0 || target_h >= h || h == 0 {
        (even(w), even(h))
    } else {
        let nw = ((w as f64 * target_h as f64 / h as f64) / 2.0).round() as u32 * 2;
        (nw, even(target_h))
    }
}

pub fn candidates(t: &Target, s: &CaptureSettings, enc: &EncoderInfo) -> Vec<Candidate> {
    let fps = s.fps.clamp(10, 240);
    let mut v = Vec::new();

    // Windows.Graphics.Capture: масштабирование делает сам WGC на GPU.
    let gfx = match t {
        Target::Monitor(m) => {
            let (w, h) = out_size(m.width, m.height, s.height);
            format!(
                "gfxcapture=hmonitor={}:max_framerate={fps}:width={w}:height={h}:resize_mode=scale_aspect",
                m.hmonitor
            )
        }
        Target::Window(wi) => {
            if s.height > 0 && (s.height as i32) < wi.height {
                let (w, h) = out_size(wi.width as u32, wi.height as u32, s.height);
                format!("gfxcapture=hwnd={}:max_framerate={fps}:width={w}:height={h}:resize_mode=scale_aspect", wi.hwnd)
            } else {
                format!("gfxcapture=hwnd={}:max_framerate={fps}:width=-2:height=-2", wi.hwnd)
            }
        }
    };

    // 1. Zero-copy для конкретного вендора.
    match (enc.vendor.as_str(), t) {
        ("amf", Target::Monitor(m)) => {
            let (w, h) = out_size(m.width, m.height, s.height);
            let scale = if (w, h) != (even(m.width), even(m.height)) {
                format!(",vpp_amf=w={w}:h={h}")
            } else {
                String::new()
            };
            v.push(Candidate {
                label: "AMD AMF — захват и кодирование в видеопамяти".into(),
                zero_copy: true,
                graph: format!("vsrc_amf=monitor_index={}:framerate={fps}{scale}", m.index),
            });
        }
        ("nvenc", _) => v.push(Candidate {
            label: "NVENC — D3D11 без копирования".into(),
            zero_copy: true,
            graph: gfx.clone(),
        }),
        ("qsv", _) => v.push(Candidate {
            label: "QuickSync — D3D11 без копирования".into(),
            zero_copy: true,
            graph: format!("{gfx},hwmap=derive_device=qsv,format=qsv"),
        }),
        _ => {}
    }

    // 2. Универсальный путь: WGC + копирование кадра в ОЗУ.
    let sw_fmt = match (enc.hardware, enc.vendor.as_str()) {
        (true, "qsv") => ",format=nv12",
        (true, _) => "",
        (false, _) => ",format=yuv420p",
    };
    v.push(Candidate {
        label: "Windows.Graphics.Capture".into(),
        zero_copy: false,
        graph: format!("{gfx},hwdownload,format=bgra{sw_fmt}"),
    });

    // 3. DXGI Desktop Duplication (только монитор).
    if let Target::Monitor(m) = t {
        let (w, h) = out_size(m.width, m.height, s.height);
        let scale = if (w, h) != (even(m.width), even(m.height)) {
            format!(",scale={w}:{h}:flags=fast_bilinear")
        } else {
            String::new()
        };
        v.push(Candidate {
            label: "DXGI Desktop Duplication".into(),
            zero_copy: false,
            graph: format!("ddagrab=output_idx={}:framerate={fps},hwdownload,format=bgra{scale}{sw_fmt}", m.index),
        });
    }
    v
}

fn push(a: &mut Vec<String>, xs: &[&str]) {
    a.extend(xs.iter().map(|s| s.to_string()));
}

/// Энкодер для потоковой записи: фиксированный битрейт с ограничением пика,
/// ключевой кадр каждую секунду (точка нарезки буфера), заголовки SPS/PPS у каждого IDR.
pub fn encoder_args(enc: &EncoderInfo, kbps: u32, fps: u32) -> Vec<String> {
    let mut a = vec!["-c:v".to_string(), enc.name.clone()];
    let (b, max, buf) = (format!("{kbps}k"), format!("{}k", kbps * 3 / 2), format!("{}k", kbps * 2));
    let g = fps.to_string();
    match enc.vendor.as_str() {
        // usage=lowlatency у AMF игнорирует -g и выдаёт один IDR на весь поток — буфер нельзя резать.
        "amf" => {
            push(&mut a, &["-usage", "transcoding", "-quality", "balanced", "-rc", "vbr_peak"]);
            push(&mut a, &["-header_insertion_mode", "idr", "-forced_idr", "1"]);
        }
        "nvenc" => push(&mut a, &["-preset", "p4", "-tune", "ll", "-rc", "vbr", "-forced-idr", "1"]),
        "qsv" => push(&mut a, &["-preset", "faster"]),
        _ => match enc.name.as_str() {
            "libx265" => push(&mut a, &["-preset", "ultrafast", "-tune", "zerolatency", "-x265-params", "log-level=error"]),
            _ => push(&mut a, &["-preset", "veryfast", "-tune", "zerolatency"]),
        },
    }
    push(&mut a, &["-b:v", &b, "-maxrate", &max, "-bufsize", &buf, "-g", &g, "-bf", "0"]);
    // Ключевой кадр строго раз в секунду по времени (работает и при переменной частоте кадров WGC).
    push(&mut a, &["-force_key_frames", "expr:gte(t,n_forced*1)"]);
    a
}

/// Полная командная строка FFmpeg (без пути к exe).
/// `t0_unix_us` — момент начала общей шкалы времени (мкс от эпохи Unix): метка кадра = RTCTIME − t0.
/// Так видео не уплывает от звука, даже если источник выдаёт не ровно заявленные 60 кадров/с.
pub fn build_args(
    c: &Candidate,
    enc: &EncoderInfo,
    s: &CaptureSettings,
    audio_urls: &[String],
    t0_unix_us: u128,
) -> Vec<String> {
    let fps = s.fps.clamp(10, 240);
    let kbps = (s.bitrate_mbps.clamp(1.0, 150.0) * 1000.0) as u32;
    let mut a: Vec<String> = Vec::new();
    push(&mut a, &["-hide_banner", "-nostdin", "-loglevel", "error"]);
    push(&mut a, &[
        "-filter_complex",
        &format!("{},setpts=(RTCTIME-{t0_unix_us})/(TB*1000000)[v]", c.graph),
    ]);
    for url in audio_urls {
        push(&mut a, &[
            "-thread_queue_size", "1024", "-f", "f32le", "-ar", &RATE.to_string(), "-ac", "2", "-i", url,
        ]);
    }
    push(&mut a, &["-map", "[v]"]);
    for i in 0..audio_urls.len() {
        push(&mut a, &["-map", &format!("{i}:a")]);
    }
    a.extend(encoder_args(enc, kbps, fps));
    if !audio_urls.is_empty() {
        push(&mut a, &["-c:a", "aac", "-b:a", "160k"]);
    }
    push(&mut a, &[
        "-f", "mpegts",
        "-mpegts_start_pid", &VIDEO_PID.to_string(),
        "-mpegts_pmt_start_pid", &PMT_PID.to_string(),
        "-muxdelay", "0", "-muxpreload", "0", "-flush_packets", "1",
        "pipe:1",
    ]);
    a
}
