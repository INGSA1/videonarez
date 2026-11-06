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
}

pub struct AudioFeed {
    pub port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub error: Arc<Mutex<Option<String>>>,
}

impl AudioFeed {
    /// `t0` — общий для видео и звука момент начала шкалы времени,
    /// `offset_secs` — поправка синхронизации (плюс = звук позже).
    pub fn start(source: AudioSource, t0: Instant, offset_secs: f64) -> std::io::Result<AudioFeed> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (stop2, error2) = (stop.clone(), error.clone());
        let handle = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let Some(mut stream) = accept(&listener, &stop2) else { return };
                let _ = stream.set_nodelay(true);
                let mut clock = Clock::new(t0, offset_secs);
                if let Err(e) = capture(&source, &mut stream, &stop2, &mut clock) {
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
        Ok(AudioFeed { port, stop, handle: Some(handle), error })
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
}

impl Clock {
    fn new(start: Instant, offset: f64) -> Self {
        Clock { start, offset, written: 0 }
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
        out.write_all(a)?;
        out.write_all(b)?;
        self.written += (q.len() / BYTES_PER_FRAME) as u64;
        q.clear();
        Ok(())
    }
}

fn capture(
    source: &AudioSource,
    out: &mut TcpStream,
    stop: &AtomicBool,
    clock: &mut Clock,
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
    while !stop.load(Ordering::Relaxed) {
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
