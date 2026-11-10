//! Захват звука через WASAPI и подача в FFmpeg по локальному TCP (f32le, 48 кГц, стерео).
//!
//! Источники: весь системный звук (loopback устройства вывода), звук одного процесса
//! (Process Loopback, Windows 10 2004+ — без Discord и музыки) и микрофон.
//!
//! WASAPI в loopback-режиме не присылает пакеты, пока ничего не играет, поэтому поток
//! дописывает тишину по часам: аудиодорожка всегда идёт ровно в реальном времени и не
//! расходится с видео. Если устройство пропало — дорожка продолжается тишиной.

use serde::Serialize;
use std::collections::VecDeque;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use wasapi::{initialize_mta, AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

pub const RATE: u32 = 48_000;
const CH: usize = 2;
const BYTES_PER_FRAME: usize = 4 * CH;

#[derive(Debug, Clone)]
pub enum AudioSource {
    /// Всё, что звучит в системе (устройство вывода по умолчанию).
    System,
    /// Только указанный процесс и его дочерние процессы.
    Process(u32),
    /// Микрофон: ID устройства или устройство по умолчанию, плюс обработка голоса.
    Mic(Option<String>, MicFx),
    /// Приложение по имени exe (Discord, браузер…). Процесс может запуститься позже или
    /// перезапуститься — дорожка ждёт его тишиной и подхватывает заново.
    App(String),
}

/// Корневой процесс приложения по имени exe (родитель которого — не то же приложение),
/// чтобы Process Loopback с include_tree захватил все его дочерние процессы.
pub fn find_app_pid(exe: &str) -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut procs: Vec<(u32, u32)> = Vec::new(); // (pid, parent)
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..n]).eq_ignore_ascii_case(exe) {
                procs.push((e.th32ProcessID, e.th32ParentProcessID));
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    let pids: Vec<u32> = procs.iter().map(|p| p.0).collect();
    procs
        .iter()
        .find(|(_, parent)| !pids.contains(parent))
        .or(procs.first())
        .map(|p| p.0)
}

pub struct AudioFeed {
    pub port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    sink: Arc<Sink>,
    pub error: Arc<Mutex<Option<String>>>,
    t0: Instant,
    /// Когда в дорожке последний раз был слышимый звук (мс от t0, 0 — ни разу).
    last_sound: Arc<std::sync::atomic::AtomicU64>,
}

impl AudioFeed {
    /// Был ли в дорожке слышимый звук за последние `secs` секунд.
    pub fn audible_within(&self, secs: f64) -> bool {
        let last = self.last_sound.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        let now = self.t0.elapsed().as_millis() as u64;
        now.saturating_sub(last) as f64 <= secs * 1000.0 + 500.0
    }

    /// `t0` — общий для видео и звука момент начала шкалы времени,
    /// `offset_secs` — поправка синхронизации (плюс = звук позже).
    pub fn start(source: AudioSource, t0: Instant, offset_secs: f64) -> std::io::Result<AudioFeed> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let error2 = error.clone();
        let last_sound = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let sink = Arc::new(Sink {
            buf: Mutex::new(VecDeque::new()),
            closed: AtomicBool::new(false),
            activating: Mutex::new(None),
        });

        // Поток отправки: держит дорожку в FFmpeg строго в реальном времени по своим часам.
        // Если захват подвис (Windows долго подключает звук перезапущенного приложения,
        // меняется устройство), сюда просто не приходят данные, и уходит тишина. Раньше
        // отправка шла из потока захвата: пока он висел, FFmpeg ждал эту дорожку, и вместе
        // с ней вставало видео — запись проседала до нескольких кадров в секунду.
        let (stop_w, sink_w, last_w) = (stop.clone(), sink.clone(), last_sound.clone());
        let writer = std::thread::Builder::new().name("audio-send".into()).spawn(move || {
            let Some(mut stream) = accept(&listener, &stop_w) else {
                sink_w.closed.store(true, Ordering::Relaxed);
                return;
            };
            let _ = stream.set_nodelay(true);
            let mut clock = Clock::new(t0, offset_secs, last_w);
            let mut q: VecDeque<u8> = VecDeque::new();
            while !stop_w.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                q.extend(sink_w.buf.lock().unwrap().drain(..));
                if clock.pad(&mut stream, &mut q).is_err() {
                    break; // FFmpeg закрыл соединение
                }
            }
            sink_w.closed.store(true, Ordering::Relaxed);
        })?;

        let (stop2, sink2) = (stop.clone(), sink.clone());
        let done = move || stop2.load(Ordering::Relaxed) || sink2.closed.load(Ordering::Relaxed);
        let sink2 = sink.clone();
        let handle = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                if let AudioSource::App(exe) = &source {
                    // Приложение: ждём процесс, пишем его звук, после закрытия снова ждём.
                    while !done() {
                        match find_app_pid(exe) {
                            Some(pid) => {
                                if let Err(e) = capture(&AudioSource::Process(pid), &sink2, &done, Some(pid)) {
                                    *error2.lock().unwrap() = Some(e.to_string());
                                    sleep_unless(&done, Duration::from_secs(1));
                                }
                            }
                            // Нет процесса — через 2 секунды ищем снова (тишину дописывает отправка).
                            None => sleep_unless(&done, Duration::from_secs(2)),
                        }
                    }
                    return;
                }
                if let Err(e) = capture(&source, &sink2, &done, None) {
                    if !done() {
                        *error2.lock().unwrap() = Some(e.to_string());
                    }
                }
                // Устройство недоступно — дорожка продолжается тишиной из потока отправки.
            })?;
        Ok(AudioFeed { port, stop, handle: Some(handle), writer: Some(writer), error, t0, last_sound, sink })
    }

    /// Захват висит в подключении к устройству дольше 3 с — сколько секунд.
    pub fn stuck_secs(&self) -> Option<f64> {
        let since = (*self.sink.activating.lock().unwrap())?;
        let s = since.elapsed().as_secs_f64();
        (s > 3.0).then_some(s)
    }

    pub fn url(&self) -> String {
        format!("tcp://127.0.0.1:{}", self.port)
    }
}

impl Drop for AudioFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Ждём только отправку (выходит за ~10 мс). Поток захвата не ждём: если он завис в
        // подключении к устройству, остановка не должна зависнуть вместе с ним — он сам
        // выйдет по флагу stop.
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
        drop(self.handle.take());
    }
}

/// Звук от потока захвата к потоку отправки.
struct Sink {
    buf: Mutex<VecDeque<u8>>,
    /// FFmpeg отключился — захват можно заканчивать.
    closed: AtomicBool,
    /// С какого момента идёт подключение к устройству или процессу (None — не подключаемся).
    activating: Mutex<Option<Instant>>,
}

impl Sink {
    fn push(&self, q: &mut VecDeque<u8>) {
        let mut b = self.buf.lock().unwrap();
        b.extend(q.drain(..));
        // Пока FFmpeg не подключился, держим не больше 2 секунд.
        let max = RATE as usize * BYTES_PER_FRAME * 2;
        if b.len() > max {
            let cut = (b.len() - max) / BYTES_PER_FRAME * BYTES_PER_FRAME;
            b.drain(..cut);
        }
    }
}

fn sleep_unless(done: &dyn Fn() -> bool, d: Duration) {
    let until = Instant::now() + d;
    while Instant::now() < until && !done() {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn accept(listener: &TcpListener, stop: &AtomicBool) -> Option<TcpStream> {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        match listener.accept() {
            Ok((s, _)) => {
                let _ = s.set_nonblocking(false);
                return Some(s);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    None
}

/// Часы аудиодорожки, привязанные к той же точке t0, что и метки видео (setpts=RTCTIME−t0).
/// Номер сэмпла = (сейчас − t0 + поправка) × 48000: если данных не хватает — добавляется тишина,
/// если устройство отдало больше (его кварц спешит) — лишнее отбрасывается. Дрейфа нет.
struct Clock {
    start: Instant,
    offset: f64,
    written: u64,
    last_sound: Arc<std::sync::atomic::AtomicU64>,
}

impl Clock {
    fn new(start: Instant, offset: f64, last_sound: Arc<std::sync::atomic::AtomicU64>) -> Self {
        Clock { start, offset, written: 0, last_sound }
    }

    /// Отдать накопленные сэмплы, удерживая дорожку на шкале реального времени.
    fn pad(&mut self, out: &mut TcpStream, q: &mut VecDeque<u8>) -> std::io::Result<()> {
        let expected = ((self.start.elapsed().as_secs_f64() + self.offset) * RATE as f64).max(0.0) as u64;
        let have = self.written + (q.len() / BYTES_PER_FRAME) as u64;
        let tol = RATE as u64 / 50; // 20 мс
        if have + tol < expected {
            let missing = (expected - have) as usize;
            q.extend(std::iter::repeat(0u8).take(missing * BYTES_PER_FRAME));
        } else if have > expected + tol * 2 {
            let extra = ((have - expected) as usize).min(q.len() / BYTES_PER_FRAME);
            q.drain(..extra * BYTES_PER_FRAME);
        }
        let (a, b) = q.as_slices();
        // Слышимый звук: любой сэмпл громче примерно −66 дБ.
        let loud = |s: &[u8]| s.chunks_exact(4).any(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]).abs() > 0.0005);
        if loud(a) || loud(b) {
            self.last_sound.store((self.start.elapsed().as_millis() as u64).max(1), Ordering::Relaxed);
        }
        out.write_all(a)?;
        out.write_all(b)?;
        self.written += (q.len() / BYTES_PER_FRAME) as u64;
        q.clear();
        Ok(())
    }
}

/// `watch_pid` — для звука процесса: выйти (Ok), когда процесс завершится, чтобы
/// вызывающий код переключился на тишину и дождался нового запуска приложения.
/// `done` — пора заканчивать (захват остановлен или FFmpeg отключился).
fn capture(
    source: &AudioSource,
    sink: &Sink,
    done: &dyn Fn() -> bool,
    watch_pid: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    initialize_mta().ok()?;
    let fmt = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, CH, None);

    // Подключение может зависнуть внутри Windows — помечаем его, чтобы показать в статусе.
    *sink.activating.lock().unwrap() = Some(Instant::now());
    struct Clear<'a>(&'a Sink);
    impl Drop for Clear<'_> {
        fn drop(&mut self) {
            *self.0.activating.lock().unwrap() = None;
        }
    }
    let activating = Clear(sink);

    let mut client = match source {
        AudioSource::System => DeviceEnumerator::new()?
            .get_default_device(&Direction::Render)?
            .get_iaudioclient()?,
        AudioSource::Process(pid) => AudioClient::new_application_loopback_client(*pid, true)?,
        AudioSource::Mic(id, _) => {
            let en = DeviceEnumerator::new()?;
            match id {
                Some(id) if !id.is_empty() => en.get_device(id)?,
                _ => en.get_default_device(&Direction::Capture)?,
            }
            .get_iaudioclient()?
        }
        AudioSource::App(_) => return Err("источник приложения обрабатывается выше".into()),
    };
    let buffer_hns = match source {
        AudioSource::Process(_) => 0,
        _ => client.get_device_period().map(|(def, _)| def).unwrap_or(100_000),
    };
    client.initialize_client(
        &fmt,
        &Direction::Capture,
        &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: buffer_hns },
    )?;
    let event = client.set_get_eventhandle()?;
    let cap = client.get_audiocaptureclient()?;
    client.start_stream()?;
    drop(activating);

    let mut q: VecDeque<u8> = VecDeque::with_capacity(RATE as usize * BYTES_PER_FRAME / 10);
    let mut last_check = Instant::now();
    // Обработка голоса: пакеты микрофона сначала копятся в `raw`, а в дорожку уходит
    // уже очищенный звук кадрами по 10 мс.
    let mut fx = match source {
        AudioSource::Mic(_, f) if f.noise || f.echo => Some(MicProc::new(*f)),
        _ => None,
    };
    let mut raw: VecDeque<u8> = VecDeque::new();
    while !done() {
        if let Some(pid) = watch_pid {
            if last_check.elapsed() > Duration::from_secs(1) {
                last_check = Instant::now();
                if !crate::capture::games::alive(pid) {
                    let _ = client.stop_stream();
                    return Ok(());
                }
            }
        }
        let _ = event.wait_for_event(20); // таймаут = тишина, это нормально
        let dst = if fx.is_some() { &mut raw } else { &mut q };
        while let Some(n) = cap.get_next_packet_size()? {
            if n == 0 {
                break;
            }
            let before = dst.len();
            let info = cap.read_from_device_to_deque(dst)?;
            if info.flags.silent {
                for b in dst.range_mut(before..) {
                    *b = 0;
                }
            }
        }
        if let Some(p) = fx.as_mut() {
            p.process(&mut raw, &mut q);
        }
        sink.push(&mut q);
    }
    let _ = client.stop_stream();
    Ok(())
}

/// Обработка голоса в дорожке микрофона.
#[derive(Debug, Clone, Copy, Default)]
pub struct MicFx {
    /// Шумоподавление: вентилятор, клавиатура, фоновый гул.
    pub noise: bool,
    /// Эхоподавление: убирает из микрофона звук игры и собеседников, попавший из колонок.
    pub echo: bool,
}

/// 10 мс при 48 кГц — размер кадра обработки WebRTC.
const FX_FRAME: usize = RATE as usize / 100;

/// Шумо- и эхоподавление WebRTC (sonora). Для эхоподавления нужен образец того, что играет
/// в колонках: открывается loopback устройства вывода по умолчанию, и каждый его кадр
/// подаётся в обработку до соответствующего кадра микрофона.
struct MicProc {
    apm: sonora::AudioProcessing,
    /// Моно-сэмплы микрофона, ещё не набравшие кадр.
    cap: Vec<f32>,
    /// Моно-сэмплы колонок для эхоподавления.
    ren: Vec<f32>,
    loopback: Option<(AudioClient, wasapi::AudioCaptureClient)>,
    ren_raw: VecDeque<u8>,
    out_frame: Vec<f32>,
}

impl MicProc {
    fn new(fx: MicFx) -> Self {
        use sonora::config::{EchoCanceller, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel};
        let config = sonora::Config {
            high_pass_filter: Some(HighPassFilter::default()),
            echo_canceller: fx.echo.then(EchoCanceller::default),
            noise_suppression: fx.noise.then(|| NoiseSuppression { level: NoiseSuppressionLevel::High, ..Default::default() }),
            ..Default::default()
        };
        let sc = sonora::StreamConfig::new(RATE, 1);
        let apm = sonora::AudioProcessing::builder().config(config).capture_config(sc).render_config(sc).build();
        let loopback = if fx.echo { open_loopback().map_err(|e| eprintln!("эхоподавление без образца колонок: {e}")).ok() } else { None };
        MicProc {
            apm,
            cap: Vec::with_capacity(FX_FRAME * 4),
            ren: Vec::with_capacity(FX_FRAME * 4),
            loopback,
            ren_raw: VecDeque::new(),
            out_frame: vec![0.0; FX_FRAME],
        }
    }

    /// Забирает сырые стерео-байты микрофона из `raw`, кладёт обработанные в `q`.
    fn process(&mut self, raw: &mut VecDeque<u8>, q: &mut VecDeque<u8>) {
        // Сначала всё, что успело прозвучать в колонках, — эхоподавлению нужен образец заранее.
        if let Some((_, lc)) = &self.loopback {
            while let Ok(Some(n)) = lc.get_next_packet_size() {
                if n == 0 || lc.read_from_device_to_deque(&mut self.ren_raw).is_err() {
                    break;
                }
            }
            downmix(&mut self.ren_raw, &mut self.ren);
            let mut off = 0;
            while self.ren.len() - off >= FX_FRAME {
                let _ = self.apm.process_render_f32(&[&self.ren[off..off + FX_FRAME]], &mut [&mut self.out_frame[..]]);
                off += FX_FRAME;
            }
            self.ren.drain(..off);
        }
        downmix(raw, &mut self.cap);
        let mut off = 0;
        while self.cap.len() - off >= FX_FRAME {
            if self.apm.process_capture_f32(&[&self.cap[off..off + FX_FRAME]], &mut [&mut self.out_frame[..]]).is_err() {
                self.out_frame.copy_from_slice(&self.cap[off..off + FX_FRAME]);
            }
            for &s in &self.out_frame {
                let b = s.clamp(-1.0, 1.0).to_le_bytes();
                q.extend(b);
                q.extend(b);
            }
            off += FX_FRAME;
        }
        self.cap.drain(..off);
    }
}

impl Drop for MicProc {
    fn drop(&mut self) {
        if let Some((c, _)) = &self.loopback {
            let _ = c.stop_stream();
        }
    }
}

/// Стерео f32 (байты) → моно, целые кадры; хвост неполного кадра остаётся в `raw`.
fn downmix(raw: &mut VecDeque<u8>, mono: &mut Vec<f32>) {
    let frames = raw.len() / BYTES_PER_FRAME;
    let bytes: Vec<u8> = raw.drain(..frames * BYTES_PER_FRAME).collect();
    for f in bytes.chunks_exact(BYTES_PER_FRAME) {
        let l = f32::from_le_bytes([f[0], f[1], f[2], f[3]]);
        let r = f32::from_le_bytes([f[4], f[5], f[6], f[7]]);
        mono.push((l + r) * 0.5);
    }
}

/// Loopback устройства вывода по умолчанию — образец звука колонок для эхоподавления.
fn open_loopback() -> Result<(AudioClient, wasapi::AudioCaptureClient), Box<dyn std::error::Error>> {
    let fmt = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, CH, None);
    let mut client = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?.get_iaudioclient()?;
    client.initialize_client(
        &fmt,
        &Direction::Capture,
        &StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: 1_000_000 },
    )?;
    let cap = client.get_audiocaptureclient()?;
    client.start_stream()?;
    Ok((client, cap))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioApp {
    pub exe: String,
    pub name: String,
    /// Прямо сейчас воспроизводит звук.
    pub playing: bool,
    /// Есть звуковая сессия (приложение уже выводило звук).
    pub has_session: bool,
}

/// Понятное имя дорожки по exe.
pub fn app_display_name(exe: &str) -> String {
    let stem = exe.trim_end_matches(".exe").trim_end_matches(".EXE");
    let known = match stem.to_lowercase().as_str() {
        "discord" | "discordptb" | "discordcanary" => Some("Discord"),
        "chrome" => Some("Chrome"),
        "msedge" => Some("Edge"),
        "firefox" => Some("Firefox"),
        "opera" => Some("Opera"),
        "browser" => Some("Яндекс Браузер"),
        "telegram" => Some("Telegram"),
        "spotify" => Some("Spotify"),
        "steam" | "steamwebhelper" => Some("Steam"),
        "ts3client_win64" | "teamspeak" => Some("TeamSpeak"),
        "yandexmusic" | "яндекс музыка" => Some("Яндекс Музыка"),
        "vlc" => Some("VLC"),
        _ => None,
    };
    match known {
        Some(k) => k.to_string(),
        None => {
            let mut c = stem.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// Приложения, которые выводят звук (сессии микшера Windows), плюс программы с открытыми окнами.
pub fn list_audio_apps() -> Vec<AudioApp> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, AudioApp> = BTreeMap::new();
    let own = std::process::id();
    let run = |map: &mut BTreeMap<String, AudioApp>| -> Result<(), Box<dyn std::error::Error>> {
        let _ = initialize_mta();
        let dev = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?;
        let en = dev.get_iaudiosessionmanager()?.get_audiosessionenumerator()?;
        for i in 0..en.get_count()? {
            let Ok(s) = en.get_session(i) else { continue };
            let pid = s.get_process_id().unwrap_or(0);
            if pid == 0 || pid == own {
                continue; // системные звуки Windows
            }
            let exe = crate::capture::sources::process_exe(pid);
            if exe.is_empty() {
                continue;
            }
            let playing = matches!(s.get_state(), Ok(wasapi::SessionState::Active));
            let e = map.entry(exe.to_lowercase()).or_insert_with(|| AudioApp {
                name: app_display_name(&exe),
                exe: exe.clone(),
                playing: false,
                has_session: true,
            });
            e.playing |= playing;
        }
        Ok(())
    };
    let _ = run(&mut map);
    for w in crate::capture::sources::windows() {
        if w.exe.is_empty() || w.exe.eq_ignore_ascii_case("explorer.exe") {
            continue;
        }
        map.entry(w.exe.to_lowercase()).or_insert_with(|| AudioApp {
            name: app_display_name(&w.exe),
            exe: w.exe.clone(),
            playing: false,
            has_session: false,
        });
    }
    let mut v: Vec<AudioApp> = map.into_values().collect();
    v.sort_by(|a, b| (!a.playing, !a.has_session, a.name.to_lowercase()).cmp(&(!b.playing, !b.has_session, b.name.to_lowercase())));
    v
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// Список микрофонов.
pub fn list_inputs() -> Vec<AudioDevice> {
    let run = || -> Result<Vec<AudioDevice>, Box<dyn std::error::Error>> {
        let _ = initialize_mta();
        let en = DeviceEnumerator::new()?;
        let default_id = en
            .get_default_device(&Direction::Capture)
            .and_then(|d| d.get_id())
            .unwrap_or_default();
        let col = en.get_device_collection(&Direction::Capture)?;
        let mut v = Vec::new();
        for i in 0..col.get_nbr_devices()? {
            let d = col.get_device_at_index(i)?;
            let id = d.get_id()?;
            v.push(AudioDevice {
                name: d.get_friendlyname().unwrap_or_else(|_| id.clone()),
                default: id == default_id,
                id,
            });
        }
        Ok(v)
    };
    run().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Шумоподавление: длина потока сохраняется, ровный шум заметно тише.
    #[test]
    fn noise_suppression_keeps_length_and_quiets_noise() {
        let mut p = MicProc::new(MicFx { noise: true, echo: false });
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.1
        };
        let (mut in_e, mut out_e, mut out_n) = (0.0f64, 0.0f64, 0usize);
        let mut q = VecDeque::new();
        for chunk in 0..300 {
            // куски по 7 мс, не кратные кадру обработки
            let mut raw = VecDeque::new();
            for _ in 0..336 {
                let s = noise();
                if chunk >= 150 {
                    in_e += (s * s) as f64;
                }
                raw.extend(s.to_le_bytes());
                raw.extend(s.to_le_bytes());
            }
            q.clear();
            p.process(&mut raw, &mut q);
            let b: Vec<u8> = q.iter().copied().collect();
            for f in b.chunks_exact(8) {
                let s = f32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                if chunk >= 150 {
                    out_e += (s * s) as f64;
                }
                out_n += 1;
            }
        }
        assert!(out_n >= 300 * 336 - FX_FRAME && out_n <= 300 * 336, "кадров: {out_n}");
        assert!(out_e < in_e * 0.5, "шум не приглушён: {in_e} -> {out_e}");
    }

    /// Эхоподавление на настоящих устройствах: loopback колонок открывается, микрофон
    /// обрабатывается кадрами без ошибок. Запуск: cargo test -- --ignored echo_live
    #[test]
    #[ignore]
    fn echo_live() {
        initialize_mta().ok().unwrap();
        let mut p = MicProc::new(MicFx { noise: true, echo: true });
        assert!(p.loopback.is_some(), "loopback колонок не открылся");
        let t = Instant::now();
        let mut q = VecDeque::new();
        let mut n = 0usize;
        while t.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
            let mut raw: VecDeque<u8> = std::iter::repeat(0u8).take(480 * BYTES_PER_FRAME).collect();
            p.process(&mut raw, &mut q);
            n += q.len() / BYTES_PER_FRAME;
            q.clear();
        }
        println!("обработано кадров: {n}, остаток колонок: {}", p.ren.len());
        assert!(n > 0);
    }

    /// Скорость каждой звуковой дорожки: должна быть 48000 кадров/с, иначе FFmpeg ждёт её и
    /// тормозит видео. Запуск: cargo test --release -- --ignored feed_rates --nocapture
    #[test]
    #[ignore]
    fn feed_rates() {
        use std::io::Read;
        let srcs = vec![
            ("система", AudioSource::System),
            ("Discord", AudioSource::App("Discord.exe".into())),
            ("Яндекс Музыка", AudioSource::App("Яндекс Музыка.exe".into())),
            ("микрофон", AudioSource::Mic(None, MicFx { noise: true, echo: false })),
            ("микрофон+эхо", AudioSource::Mic(None, MicFx { noise: true, echo: true })),
        ];
        let t0 = Instant::now();
        let mut hs = Vec::new();
        for (name, s) in srcs {
            let feed = AudioFeed::start(s, t0, 0.0).unwrap();
            let port = feed.port;
            hs.push(std::thread::spawn(move || {
                let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
                let start = Instant::now();
                let mut buf = vec![0u8; 65536];
                let mut total = 0usize;
                let mut worst = Duration::ZERO;
                let mut last = Instant::now();
                sock.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
                while start.elapsed() < Duration::from_secs(10) {
                    match sock.read(&mut buf) {
                        Ok(n) if n > 0 => {
                            total += n;
                            worst = worst.max(last.elapsed());
                            last = Instant::now();
                        }
                        _ => worst = worst.max(last.elapsed()),
                    }
                }
                let rate = total as f64 / BYTES_PER_FRAME as f64 / start.elapsed().as_secs_f64();
                let err = feed.error.lock().unwrap().clone();
                println!("{name}: {rate:.0} кадров/с, самая длинная пауза {} мс, ошибка: {err:?}", worst.as_millis());
                drop(feed);
            }));
        }
        for h in hs {
            h.join().unwrap();
        }
    }
}