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
    /// Микрофон: ID устройства или устройство по умолчанию.
    Mic(Option<String>),
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
        let (stop2, error2) = (stop.clone(), error.clone());
        let last_sound = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let last2 = last_sound.clone();
        let handle = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let Some(mut stream) = accept(&listener, &stop2) else { return };
                let _ = stream.set_nodelay(true);
                let mut clock = Clock::new(t0, offset_secs, last2);
                if let AudioSource::App(exe) = &source {
                    // Приложение: ждём процесс, пишем его звук, после закрытия снова ждём.
                    while !stop2.load(Ordering::Relaxed) {
                        match find_app_pid(exe) {
                            Some(pid) => {
                                if let Err(e) = capture(&AudioSource::Process(pid), &mut stream, &stop2, &mut clock, Some(pid)) {
                                    if is_closed(&e) {
                                        return;
                                    }
                                    *error2.lock().unwrap() = Some(e.to_string());
                                }
                            }
                            None => {
                                // Нет процесса — 2 секунды тишины и снова ищем.
                                let until = Instant::now() + Duration::from_secs(2);
                                while Instant::now() < until && !stop2.load(Ordering::Relaxed) {
                                    std::thread::sleep(Duration::from_millis(20));
                                    if clock.pad(&mut stream, &mut VecDeque::new()).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    return;
                }
                if let Err(e) = capture(&source, &mut stream, &stop2, &mut clock, None) {
                    if !stop2.load(Ordering::Relaxed) {
                        *error2.lock().unwrap() = Some(e.to_string());
                    }
                }
                // Устройство недоступно — держим дорожку тишиной, пока FFmpeg читает.
                while !stop2.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));
                    if clock.pad(&mut stream, &mut VecDeque::new()).is_err() {
                        break;
                    }
                }
            })?;
        Ok(AudioFeed { port, stop, handle: Some(handle), error, t0, last_sound })
    }

    pub fn url(&self) -> String {
        format!("tcp://127.0.0.1:{}", self.port)
    }
}

impl Drop for AudioFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
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

/// FFmpeg закрыл соединение (захват остановлен) — это не ошибка устройства.
fn is_closed(e: &Box<dyn std::error::Error>) -> bool {
    e.downcast_ref::<std::io::Error>().map_or(false, |io| {
        matches!(
            io.kind(),
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
        )
    })
}

/// `watch_pid` — для звука процесса: выйти (Ok), когда процесс завершится, чтобы
/// вызывающий код переключился на тишину и дождался нового запуска приложения.
fn capture(
    source: &AudioSource,
    out: &mut TcpStream,
    stop: &AtomicBool,
    clock: &mut Clock,
    watch_pid: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    initialize_mta().ok()?;
    let fmt = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, CH, None);

    let mut client = match source {
        AudioSource::System => DeviceEnumerator::new()?
            .get_default_device(&Direction::Render)?
            .get_iaudioclient()?,
        AudioSource::Process(pid) => AudioClient::new_application_loopback_client(*pid, true)?,
        AudioSource::Mic(id) => {
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

    let mut q: VecDeque<u8> = VecDeque::with_capacity(RATE as usize * BYTES_PER_FRAME / 10);
    let mut last_check = Instant::now();
    while !stop.load(Ordering::Relaxed) {
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
        while let Some(n) = cap.get_next_packet_size()? {
            if n == 0 {
                break;
            }
            let before = q.len();
            let info = cap.read_from_device_to_deque(&mut q)?;
            if info.flags.silent {
                for b in q.range_mut(before..) {
                    *b = 0;
                }
            }
        }
        clock.pad(out, &mut q)?;
    }
    let _ = client.stop_stream();
    Ok(())
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
