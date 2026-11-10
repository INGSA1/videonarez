//! Фоновый захват: запуск FFmpeg по «лестнице» кандидатов, кольцевой буфер в ОЗУ,
//! сохранение повтора по горячей клавише, обычная запись, статус и автоперезапуск.

use super::audio::{list_inputs, AudioDevice, AudioFeed, AudioSource};
use super::games::GameInfo;
use super::pipeline::{build_args, candidates, Candidate, Target};
use super::ring::Ring;
use super::sources::{self, GpuInfo, MonitorInfo, WindowInfo};
use crate::config::gpu::{self, EncoderInfo};
use crate::config::settings::{persist, CaptureSettings};
use crate::error::{AppError, AppResult};
use crate::ffmpeg::tools;
use crate::state::AppState;
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Захват идёт с приоритетом «выше среднего», как в OBS: работы у него мало, но если игра
/// загрузит процессор целиком, захват не должен оставаться без времени.
const ABOVE_NORMAL_PRIORITY: u32 = 0x0000_8000;
const BELOW_NORMAL_PRIORITY: u32 = 0x0000_4000;

pub struct Session {
    child: Child,
    pub ring: Arc<Mutex<Ring>>,
    _audio: Vec<AudioFeed>,
    audio_errors: Vec<Arc<Mutex<Option<String>>>>,
    reader: Option<JoinHandle<()>>,
    stderr: Arc<Mutex<String>>,
    pub started: Instant,
    pub pipeline: String,
    pub zero_copy: bool,
    pub encoder: String,
    pub source: String,
    pub prefix: String,
    pub tracks: Vec<String>,
    pub ffmpeg_pid: u32,
    pub gpu_priority: Option<&'static str>,
    stop_wait: Duration,
}

/// Мягкая остановка: «q» → FFmpeg сам корректно закрывает захват и аппаратный энкодер.
/// Резкое убийство процесса посреди захвата на уровне драйвера (vsrc_amf, ddagrab) опасно
/// для видеодрайвера, поэтому kill — только если процесс не завершился сам за `wait`.
/// (Замерено: vsrc_amf и ddagrab выходят по «q» за ~150 мс; gfxcapture на «q» не реагирует,
/// но это обычная пользовательская сессия WGC, и её завершение безопасно.)
fn graceful_stop(child: &mut Child, wait: Duration) {
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(b"q");
        let _ = stdin.flush();
    }
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn stop_wait(graph: &str) -> Duration {
    if graph.starts_with("gfxcapture") {
        Duration::from_millis(300)
    } else {
        Duration::from_secs(3)
    }
}

impl Session {
    /// Дорожки для клипа длиной `dur` секунд: молчавшие всё это время помечаются `None`.
    fn tracks_for(&self, dur: f64) -> Vec<Option<String>> {
        self.tracks
            .iter()
            .zip(&self._audio)
            .map(|(name, feed)| feed.audible_within(dur).then(|| name.clone()))
            .collect()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        graceful_stop(&mut self.child, self.stop_wait);
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

#[derive(Default)]
pub struct Recorder {
    pub session: Mutex<Option<Session>>,
    starting: AtomicBool,
    pub last_error: Mutex<Option<String>>,
    pub last_clip: Mutex<Option<String>>,
    stats: Mutex<Stats>,
    restarts: Mutex<Vec<Instant>>,
    /// Пользователь хочет, чтобы буфер работал (для автоперезапуска и режима «авто»).
    wanted: AtomicBool,
    /// Игра, за которой следит режим «авто».
    pub game: Mutex<Option<GameInfo>>,
    /// Клипы, сохранённые за время работы приложения (переживают пересоздание окна).
    pub recent: Mutex<Vec<ClipSaved>>,
}

impl Recorder {
    pub fn is_wanted(&self) -> bool {
        self.wanted.load(Ordering::Relaxed)
    }
}

#[derive(Default, Clone, Copy)]
struct Stats {
    at: Option<Instant>,
    frames: u64,
    bytes: u64,
    fps: f64,
    kbps: f64,
    /// Сколько секунд подряд полноэкранная игра на переднем плане, а кадров заметно меньше нужного.
    low_secs: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub running: bool,
    pub starting: bool,
    pub pipeline: String,
    pub zero_copy: bool,
    pub encoder: String,
    pub source: String,
    pub buffered_secs: f64,
    pub buffer_mb: f64,
    pub max_secs: f64,
    pub fps: f64,
    pub bitrate_kbps: f64,
    pub recording_secs: Option<f64>,
    pub uptime_secs: f64,
    pub error: Option<String>,
    pub audio_errors: Vec<String>,
    pub last_clip: Option<String>,
    pub ffmpeg_pid: Option<u32>,
    /// Режим «авто»: буфер взведён и ждёт полноэкранную игру.
    pub waiting_for_game: bool,
    pub game: Option<String>,
    /// Приоритет захвата на видеокарте (None — не удалось поднять).
    pub gpu_priority: Option<String>,
    /// Предупреждение о нехватке кадров в игре.
    pub warning: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClipSaved {
    pub path: String,
    pub duration: f64,
    pub kind: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSources {
    pub gpu: Option<GpuInfo>,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
    pub mics: Vec<AudioDevice>,
    pub audio_apps: Vec<super::audio::AudioApp>,
}

// ---------- Запуск ----------

fn resolve_target(cs: &CaptureSettings, monitors: &[MonitorInfo], game: Option<&GameInfo>) -> AppResult<Target> {
    if cs.source == "auto" {
        // Игру пишем захватом её монитора: так работает и эксклюзивный fullscreen.
        let g = game.ok_or_else(|| AppError::msg("Игра не обнаружена"))?;
        return monitors
            .iter()
            .find(|m| m.hmonitor == g.hmonitor)
            .or_else(|| monitors.first())
            .cloned()
            .map(Target::Monitor)
            .ok_or_else(|| AppError::msg("Не найден монитор игры"));
    }
    if cs.source == "window" {
        let list = sources::windows();
        let exe = cs.window_exe.to_lowercase();
        let found = list
            .iter()
            .filter(|w| w.exe.to_lowercase() == exe)
            .max_by_key(|w| (w.title == cs.window_title, w.width as i64 * w.height as i64))
            .cloned();
        if let Some(w) = &found {
            if w.width == 0 {
                return Err(AppError::msg(format!(
                    "Окно «{}» свёрнуто — Windows не отдаёт кадры свёрнутых окон. Разверните его и включите буфер снова",
                    w.title
                )));
            }
        }
        return found.map(Target::Window).ok_or_else(|| {
            AppError::msg(format!(
                "Окно «{}» не найдено — запустите приложение или выберите другой источник",
                if cs.window_title.is_empty() { &cs.window_exe } else { &cs.window_title }
            ))
        });
    }
    monitors
        .iter()
        .find(|m| m.index == cs.monitor)
        .or_else(|| monitors.first())
        .cloned()
        .map(Target::Monitor)
        .ok_or_else(|| AppError::msg("Не найден ни один монитор"))
}

pub fn start(app: &AppHandle) -> AppResult<()> {
    let state = app.state::<AppState>();
    let rec = &state.recorder;
    rec.wanted.store(true, Ordering::Relaxed);
    if rec.session.lock().unwrap().is_some() {
        return Ok(());
    }
    // Режим «авто» без игры: буфер взведён, захват начнёт следящий поток.
    if state.settings().capture.source == "auto" && rec.game.lock().unwrap().is_none() {
        *rec.last_error.lock().unwrap() = None;
        emit_status(app);
        crate::shell::refresh_tray(app);
        return Ok(());
    }
    if rec.starting.swap(true, Ordering::SeqCst) {
        return Err(AppError::msg("Захват уже запускается"));
    }
    emit_status(app);
    let res = start_inner(app);
    rec.starting.store(false, Ordering::SeqCst);
    match &res {
        Ok(()) => *rec.last_error.lock().unwrap() = None,
        Err(e) => {
            eprintln!("[capture] start failed: {e}");
            *rec.last_error.lock().unwrap() = Some(e.to_string());
            // Ручной режим: неудачный запуск снимает «взвод». В режиме «авто» ждём следующую игру.
            if state.settings().capture.source != "auto" {
                rec.wanted.store(false, Ordering::Relaxed);
            }
        }
    }
    emit_status(app);
    crate::shell::refresh_tray(app);
    res
}

fn start_inner(app: &AppHandle) -> AppResult<()> {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let cs = settings.capture.clone();
    let (_gpu, monitors) = sources::gpu_and_monitors();
    let game = if cs.source == "auto" { state.recorder.game.lock().unwrap().clone() } else { None };
    let target = resolve_target(&cs, &monitors, game.as_ref())?;
    let codec = if cs.codec == "hevc" { "hevc" } else { "h264" };
    let enc = gpu::pick(&settings.encoders, &cs.encoder, codec)
        .ok_or_else(|| AppError::msg(format!("Нет кодировщика {} для записи", codec.to_uppercase())))?;

    let mut audio: Vec<(AudioSource, String)> = Vec::new();
    if cs.system_audio {
        match (&target, &game, cs.app_audio_only) {
            (_, Some(g), true) => audio.push((AudioSource::Process(g.pid), "Игра".into())),
            (Target::Window(w), None, true) => audio.push((AudioSource::Process(w.pid), "Игра".into())),
            _ => audio.push((AudioSource::System, "Системный звук".into())),
        }
    }
    // Отдельные дорожки выбранных приложений (без дублей с дорожкой игры).
    let game_exe = game.as_ref().map(|g| g.exe.to_lowercase());
    let mut seen = std::collections::HashSet::new();
    for exe in &cs.audio_apps {
        let key = exe.trim().to_lowercase();
        if key.is_empty() || !seen.insert(key.clone()) || (cs.app_audio_only && game_exe.as_deref() == Some(key.as_str())) {
            continue;
        }
        audio.push((AudioSource::App(exe.trim().to_string()), super::audio::app_display_name(exe.trim())));
    }
    if cs.mic {
        let id = (!cs.mic_device.is_empty()).then(|| cs.mic_device.clone());
        let fx = super::audio::MicFx { noise: cs.mic_noise_suppression, echo: cs.mic_echo_cancel };
        audio.push((AudioSource::Mic(id, fx), "Микрофон".into()));
    }

    let mut errors = Vec::new();
    for cand in candidates(&target, &cs, &enc) {
        match try_start(&cand, &enc, &cs, &target, &audio) {
            Ok(mut sess) => {
                if let Some(g) = &game {
                    sess.source = format!("Игра: {}", g.name());
                    let stem = g.exe.trim_end_matches(".exe").trim_end_matches(".EXE");
                    if !stem.is_empty() {
                        sess.prefix = stem.to_string();
                    }
                }
                *state.recorder.session.lock().unwrap() = Some(sess);
                *state.recorder.stats.lock().unwrap() = Stats::default();
                return Ok(());
            }
            Err(e) => errors.push(format!("{}: {}", cand.label, e)),
        }
    }
    Err(AppError::msg(format!("Не удалось запустить захват.\n{}", errors.join("\n"))))
}

fn try_start(
    cand: &Candidate,
    enc: &EncoderInfo,
    cs: &CaptureSettings,
    target: &Target,
    audio: &[(AudioSource, String)],
) -> Result<Session, String> {
    // Общая точка отсчёта для меток видео (RTCTIME) и часов звука.
    let t0 = Instant::now();
    let t0_unix_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    let offset = cs.audio_offset_ms.clamp(-2000, 2000) as f64 / 1000.0;
    let feeds: Vec<AudioFeed> = audio
        .iter()
        .map(|(src, _)| AudioFeed::start(src.clone(), t0, offset))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("аудио: {e}"))?;
    let urls: Vec<String> = feeds.iter().map(|f| f.url()).collect();
    let args = build_args(cand, enc, cs, &urls, t0_unix_us);

    let ffmpeg = tools().map_err(|e| e.to_string())?.ffmpeg.clone();
    let mut cmd = Command::new(ffmpeg);
    cmd.args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | ABOVE_NORMAL_PRIORITY);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    // Приоритет на видеокарте: без него игра без ограничения FPS забирает GPU целиком.
    let gpu_priority = super::gpu_priority::raise(child.id());
    // Захват не должен замедляться энергосбережением Windows, когда окна свёрнуты.
    super::gpu_priority::no_power_throttling(0);
    super::gpu_priority::no_power_throttling(child.id());
    eprintln!("[capture] GPU priority: {}", gpu_priority.unwrap_or("не изменён"));

    let ring = Arc::new(Mutex::new(Ring::new(cs.buffer_secs.clamp(5, 1800) as f64)));
    let mut stdout = child.stdout.take().unwrap();
    let ring2 = ring.clone();
    let reader = std::thread::Builder::new()
        .name("capture-reader".into())
        .spawn(move || {
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => ring2.lock().unwrap().push(&buf[..n]),
                }
            }
        })
        .map_err(|e| e.to_string())?;

    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let mut stderr = child.stderr.take().unwrap();
    let sb = stderr_buf.clone();
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut b) {
            if n == 0 {
                break;
            }
            let mut s = sb.lock().unwrap();
            s.push_str(&String::from_utf8_lossy(&b[..n]));
            if s.len() > 4000 {
                let cut = s.len() - 2000;
                let cut = (cut..s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(0);
                s.drain(..cut);
            }
        }
    });

    // Ждём первый ключевой кадр.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if ring.lock().unwrap().has_keyframe() {
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            std::thread::sleep(Duration::from_millis(100));
            let err = stderr_buf.lock().unwrap().trim().to_string();
            let _ = reader.join();
            return Err(if err.is_empty() { format!("FFmpeg завершился ({status})") } else { last_line(&err) });
        }
        if Instant::now() > deadline {
            graceful_stop(&mut child, stop_wait(&cand.graph));
            let _ = reader.join();
            return Err("нет кадров за 8 секунд".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(Session {
        ffmpeg_pid: child.id(),
        gpu_priority,
        child,
        ring,
        audio_errors: feeds.iter().map(|f| f.error.clone()).collect(),
        _audio: feeds,
        reader: Some(reader),
        stderr: stderr_buf,
        started: Instant::now(),
        pipeline: cand.label.clone(),
        zero_copy: cand.zero_copy,
        encoder: enc.name.clone(),
        source: target.label(),
        prefix: target.file_prefix(),
        tracks: audio.iter().map(|(_, n)| n.clone()).collect(),
        stop_wait: stop_wait(&cand.graph),
    })
}

fn last_line(s: &str) -> String {
    s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(s).trim().to_string()
}

/// Выключить буфер совсем (кнопка, трей, выход).
pub fn stop(app: &AppHandle) {
    app.state::<AppState>().recorder.wanted.store(false, Ordering::Relaxed);
    stop_session(app);
}

/// Остановить текущий захват, не снимая «взвода» (смена игры, выход из игры, перезапуск).
pub fn stop_session(app: &AppHandle) {
    let state = app.state::<AppState>();
    let sess = state.recorder.session.lock().unwrap().take();
    if let Some(s) = sess {
        let rec = s.ring.lock().unwrap().stop_recording();
        let tracks = rec.as_ref().map(|(_, d)| s.tracks_for(*d)).unwrap_or_default();
        drop(s); // убивает FFmpeg
        if let Some((ts, dur)) = rec {
            finish_recording(app, ts, dur, tracks);
        }
    }
    emit_status(app);
    crate::shell::refresh_tray(app);
}

// ---------- Сохранение ----------

/// Повторы из буфера — в «Клипы», целые записи (Alt+F9) — в «Записи».
fn clips_dir(app: &AppHandle) -> PathBuf {
    app.state::<AppState>().settings().clips_dir()
}
fn recordings_dir(app: &AppHandle) -> PathBuf {
    app.state::<AppState>().settings().recordings_dir()
}

fn unique(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let p = dir.join(format!("{stem}.{ext}"));
    if !p.exists() {
        return p;
    }
    (2..1000)
        .map(|i| dir.join(format!("{stem} ({i}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(p)
}

/// TS → MP4 без перекодирования, с названиями звуковых дорожек.
/// `tracks[i]` — название i-й звуковой дорожки потока, `None` — дорожку не сохранять
/// (в ней за время клипа не было звука: приложение закрыто или молчало).
fn remux(ts: &Path, mp4: &Path, tracks: &[Option<String>]) -> Result<(), String> {
    let ffmpeg = tools().map_err(|e| e.to_string())?.ffmpeg.clone();
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-i"]).arg(ts);
    cmd.args(["-map", "0:v"]);
    let mut k = 0;
    for (i, t) in tracks.iter().enumerate() {
        let Some(t) = t else { continue };
        cmd.args(["-map", &format!("0:a:{i}")]);
        cmd.arg(format!("-metadata:s:a:{k}")).arg(format!("title={t}"));
        cmd.arg(format!("-metadata:s:a:{k}")).arg(format!("handler_name={t}"));
        k += 1;
    }
    cmd.args(["-c", "copy"]);
    cmd.args(["-movflags", "+faststart", "-f", "mp4"]).arg(mp4);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(crate::ffmpeg::tail(&out.stderr))
    }
}

fn notify_saved(app: &AppHandle, path: &Path, duration: f64, kind: &str) {
    let state = app.state::<AppState>();
    let p = path.to_string_lossy().into_owned();
    *state.recorder.last_clip.lock().unwrap() = Some(p.clone());
    if state.settings().capture.sound {
        sources::beep();
    }
    let clip = ClipSaved { path: p, duration, kind: kind.into() };
    {
        let mut r = state.recorder.recent.lock().unwrap();
        r.insert(0, clip.clone());
        r.truncate(50);
    }
    let _ = app.emit("clip-saved", clip);
}

/// Сохранить последние `secs` секунд буфера (None — весь буфер).
pub fn save_replay(app: &AppHandle, secs: Option<f64>) -> AppResult<()> {
    let state = app.state::<AppState>();
    let (data, dur, prefix, tracks) = {
        let g = state.recorder.session.lock().unwrap();
        let s = g.as_ref().ok_or_else(|| AppError::msg("Буфер повтора не запущен"))?;
        let ring = s.ring.lock().unwrap();
        let secs = secs.unwrap_or(ring.max_secs);
        let (data, dur) = ring.snapshot(secs).ok_or_else(|| AppError::msg("Буфер ещё пуст"))?;
        (data, dur, s.prefix.clone(), s.tracks_for(dur))
    };
    let dir = clips_dir(app);
    std::fs::create_dir_all(&dir)?;
    let app = app.clone();
    std::thread::spawn(move || {
        let stem = format!("{prefix}_{}", sources::local_stamp());
        let mp4 = unique(&dir, &stem, "mp4");
        let ts = dir.join(format!(".{stem}.ts.part"));
        let res = std::fs::write(&ts, &data)
            .map_err(|e| e.to_string())
            .and_then(|_| remux(&ts, &mp4, &tracks));
        let _ = std::fs::remove_file(&ts);
        match res {
            Ok(()) => notify_saved(&app, &mp4, dur, "replay"),
            Err(e) => {
                let _ = std::fs::remove_file(&mp4);
                *app.state::<AppState>().recorder.last_error.lock().unwrap() = Some(format!("Не удалось сохранить повтор: {e}"));
                emit_status(&app);
            }
        }
    });
    Ok(())
}

fn finish_recording(app: &AppHandle, ts: PathBuf, dur: f64, tracks: Vec<Option<String>>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mp4 = ts.with_extension("mp4");
        match remux(&ts, &mp4, &tracks) {
            Ok(()) => {
                let _ = std::fs::remove_file(&ts);
                notify_saved(&app, &mp4, dur, "recording");
            }
            // Сырой .ts остаётся на диске — он читается и после сбоя.
            Err(e) => {
                *app.state::<AppState>().recorder.last_error.lock().unwrap() =
                    Some(format!("Запись сохранена как .ts, но не перепакована в MP4: {e}"));
                emit_status(&app);
            }
        }
    });
}

/// Начать / остановить обычную запись (пишется тот же поток, что идёт в буфер).
pub fn toggle_record(app: &AppHandle) -> AppResult<bool> {
    let state = app.state::<AppState>();
    if state.recorder.session.lock().unwrap().is_none() {
        start(app)?;
    }
    let (stopped, tracks) = {
        let g = state.recorder.session.lock().unwrap();
        let s = g.as_ref().ok_or_else(|| AppError::msg("Захват не запущен"))?;
        let mut ring = s.ring.lock().unwrap();
        if ring.recording_secs().is_some() {
            let stopped = ring.stop_recording();
            let tracks = stopped.as_ref().map(|(_, d)| s.tracks_for(*d)).unwrap_or_default();
            (stopped, tracks)
        } else {
            let dir = recordings_dir(app);
            std::fs::create_dir_all(&dir)?;
            let stem = format!("{}_{}", s.prefix, sources::local_stamp());
            ring.start_recording(unique(&dir, &stem, "ts"))?;
            (None, Vec::new())
        }
    };
    let now_recording = stopped.is_none();
    if let Some((ts, dur)) = stopped {
        finish_recording(app, ts, dur, tracks);
    }
    emit_status(app);
    crate::shell::refresh_tray(app);
    Ok(now_recording)
}

// ---------- Статус ----------

pub fn status(app: &AppHandle) -> CaptureStatus {
    let state = app.state::<AppState>();
    let rec = &state.recorder;
    let stats = *rec.stats.lock().unwrap();
    let g = rec.session.lock().unwrap();
    let mut st = CaptureStatus {
        running: g.is_some(),
        starting: rec.starting.load(Ordering::Relaxed),
        pipeline: String::new(),
        zero_copy: false,
        encoder: String::new(),
        source: String::new(),
        buffered_secs: 0.0,
        buffer_mb: 0.0,
        max_secs: state.settings().capture.buffer_secs as f64,
        fps: stats.fps,
        bitrate_kbps: stats.kbps,
        recording_secs: None,
        uptime_secs: 0.0,
        error: rec.last_error.lock().unwrap().clone(),
        audio_errors: Vec::new(),
        last_clip: rec.last_clip.lock().unwrap().clone(),
        ffmpeg_pid: None,
        waiting_for_game: g.is_none()
            && rec.is_wanted()
            && state.settings().capture.source == "auto",
        game: rec.game.lock().unwrap().as_ref().map(|g| g.name()),
        gpu_priority: None,
        warning: (stats.low_secs >= 5).then(|| {
            format!(
                "В записи около {:.0} кадров/с из {}. Игра забирает видеокарту целиком: ограничьте её FPS в настройках игры (например, до 144).",
                stats.fps,
                state.settings().capture.fps
            )
        }),
    };
    if let Some(s) = g.as_ref() {
        let ring = s.ring.lock().unwrap();
        st.pipeline = s.pipeline.clone();
        st.zero_copy = s.zero_copy;
        st.encoder = s.encoder.clone();
        st.source = s.source.clone();
        st.buffered_secs = ring.buffered_secs().min(ring.max_secs);
        st.buffer_mb = ring.buffered_bytes() as f64 / 1_048_576.0;
        st.max_secs = ring.max_secs;
        st.recording_secs = ring.recording_secs();
        st.uptime_secs = s.started.elapsed().as_secs_f64();
        st.ffmpeg_pid = Some(s.ffmpeg_pid);
        st.gpu_priority = s.gpu_priority.map(String::from);
        st.audio_errors = s
            .audio_errors
            .iter()
            .zip(&s.tracks)
            .filter_map(|(e, t)| e.lock().unwrap().as_ref().map(|e| format!("{t}: {e}")))
            .collect();
        for (feed, t) in s._audio.iter().zip(&s.tracks) {
            if let Some(secs) = feed.stuck_secs() {
                st.audio_errors.push(format!("{t}: Windows не отдаёт звук уже {secs:.0} с, дорожка пишется тишиной"));
            }
        }
        if let Some(e) = &ring.rec_error {
            st.error = Some(format!("Ошибка записи на диск: {e}"));
        }
        if ring.no_keyframes {
            st.error = Some("Кодировщик не выдаёт ключевые кадры — буфер сброшен для защиты памяти. Попробуйте другой кодировщик.".into());
        }
    }
    st
}

pub fn emit_status(app: &AppHandle) {
    let _ = app.emit("capture-status", status(app));
}

/// Фоновый поток: раз в секунду — FPS/битрейт, контроль жизни FFmpeg, автоперезапуск.
pub fn spawn_monitor(app: AppHandle) {
    std::thread::Builder::new()
        .name("capture-monitor".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let state = app.state::<AppState>();
            let rec = &state.recorder;
            let mut died: Option<String> = None;
            {
                let mut g = rec.session.lock().unwrap();
                if let Some(s) = g.as_mut() {
                    if let Ok(Some(code)) = s.child.try_wait() {
                        let err = last_line(s.stderr.lock().unwrap().trim());
                        died = Some(if err.is_empty() { format!("FFmpeg остановился ({code})") } else { err });
                    } else {
                        let (frames, bytes) = {
                            let r = s.ring.lock().unwrap();
                            (r.frames, r.total_bytes)
                        };
                        let mut st = rec.stats.lock().unwrap();
                        let now = Instant::now();
                        if let Some(at) = st.at {
                            let dt = now.duration_since(at).as_secs_f64().max(0.001);
                            st.fps = (frames - st.frames) as f64 / dt;
                            st.kbps = (bytes - st.bytes) as f64 * 8.0 / 1000.0 / dt;
                        }
                        // Нехватка кадров считается, только когда на переднем плане полноэкранная игра:
                        // на рабочем столе низкая частота — это просто неподвижная картинка.
                        let cs = state.settings().capture;
                        let starving = st.at.is_some()
                            && st.fps < cs.fps as f64 * 0.6
                            && super::games::foreground_fullscreen(&cs.game_exclude).is_some();
                        st.low_secs = if starving { st.low_secs + 1 } else { 0 };
                        *st = Stats { at: Some(now), frames, bytes, ..*st };
                    }
                }
                if died.is_some() {
                    if let Some(s) = g.take() {
                        let rec_file = s.ring.lock().unwrap().stop_recording();
                        let tracks = rec_file.as_ref().map(|(_, d)| s.tracks_for(*d)).unwrap_or_default();
                        drop(s);
                        if let Some((ts, dur)) = rec_file {
                            finish_recording(&app, ts, dur, tracks);
                        }
                    }
                }
            }
            if let Some(e) = died {
                eprintln!("[capture] ffmpeg died: {e}");
                *rec.last_error.lock().unwrap() = Some(format!("Захват прервался: {e}"));
                // Не больше 3 перезапусков в минуту (смена разрешения в игре, переключение fullscreen…).
                let allow = {
                    let mut r = rec.restarts.lock().unwrap();
                    r.retain(|t| t.elapsed() < Duration::from_secs(60));
                    r.push(Instant::now());
                    r.len() <= 3
                };
                // В режиме «авто» без живой игры перезапуск не нужен — следящий поток дождётся новой.
                let game_ok = state.settings().capture.source != "auto"
                    || rec.game.lock().unwrap().as_ref().map_or(false, |g| super::games::alive(g.pid));
                if allow && game_ok && rec.wanted.load(Ordering::Relaxed) {
                    let _ = start(&app);
                }
                crate::shell::refresh_tray(&app);
            }
            emit_status(&app);
        })
        .expect("spawn capture monitor");
}

// ---------- Команды ----------

#[tauri::command]
pub fn capture_sources() -> CaptureSources {
    let (gpu, monitors) = sources::gpu_and_monitors();
    CaptureSources {
        gpu,
        monitors,
        windows: sources::windows(),
        mics: list_inputs(),
        audio_apps: super::audio::list_audio_apps(),
    }
}

#[tauri::command]
pub async fn capture_start(app: AppHandle) -> AppResult<CaptureStatus> {
    let a = app.clone();
    tauri::async_runtime::spawn_blocking(move || start(&a))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    Ok(status(&app))
}

#[tauri::command]
pub async fn capture_stop(app: AppHandle) -> CaptureStatus {
    let a = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || stop(&a)).await;
    status(&app)
}

#[tauri::command]
pub fn capture_status(app: AppHandle) -> CaptureStatus {
    status(&app)
}

#[tauri::command]
pub fn capture_recent(state: State<'_, AppState>) -> Vec<ClipSaved> {
    state.recorder.recent.lock().unwrap().clone()
}

#[tauri::command]
pub fn capture_save(app: AppHandle, secs: Option<f64>) -> AppResult<()> {
    save_replay(&app, secs)
}

#[tauri::command]
pub async fn capture_toggle_record(app: AppHandle) -> AppResult<bool> {
    let a = app.clone();
    tauri::async_runtime::spawn_blocking(move || toggle_record(&a))
        .await
        .map_err(|e| AppError::msg(e.to_string()))?
}

/// Сохранить настройки захвата, перерегистрировать хоткеи и перезапустить захват, если он шёл.
#[tauri::command]
pub async fn capture_apply_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    capture: CaptureSettings,
) -> AppResult<Vec<String>> {
    {
        state.settings.write().unwrap().capture = capture;
    }
    persist(&state)?;
    let hotkey_errors = crate::shell::register_hotkeys(&app);
    let running = state.recorder.session.lock().unwrap().is_some();
    if running {
        let a = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            stop(&a);
            start(&a)
        })
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    }
    Ok(hotkey_errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Тот же путь, что в приложении (try_start: звук по TCP → FFmpeg → кольцевой буфер),
    /// печатает частоту кадров каждые 5 с. Запуск: cargo test --release -- --ignored live_fps --nocapture
    #[test]
    #[ignore]
    fn live_fps() {
        let secs: u64 = std::env::var("LIVE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
        let cs = CaptureSettings { mic: true, ..Default::default() };
        let (_g, monitors) = sources::gpu_and_monitors();
        let target = Target::Monitor(monitors[0].clone());
        let enc = EncoderInfo { name: "h264_amf".into(), vendor: "amf".into(), codec: "h264".into(), hardware: true };
        let audio = vec![
            (AudioSource::System, "Системный звук".to_string()),
            (AudioSource::App("Discord.exe".into()), "Discord".into()),
            (AudioSource::App("Яндекс Музыка.exe".into()), "Яндекс Музыка".into()),
            (AudioSource::Mic(None, super::super::audio::MicFx { noise: true, echo: false }), "Микрофон".into()),
        ];
        let cand = candidates(&target, &cs, &enc).into_iter().find(|c| c.label.starts_with("DXGI")).unwrap();
        println!("цепочка: {}", cand.label);
        let sess = try_start(&cand, &enc, &cs, &target, &audio).unwrap();
        let start = Instant::now();
        let mut last = (0u64, Instant::now());
        while start.elapsed() < Duration::from_secs(secs) {
            std::thread::sleep(Duration::from_secs(5));
            let frames = sess.ring.lock().unwrap().frames;
            let dt = last.1.elapsed().as_secs_f64();
            println!("{:>4} с: {:.1} кадров/с", start.elapsed().as_secs(), (frames - last.0) as f64 / dt);
            last = (frames, Instant::now());
        }
        drop(sess);
    }
}
