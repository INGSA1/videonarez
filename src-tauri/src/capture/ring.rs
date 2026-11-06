//! Кольцевой буфер повтора в оперативной памяти.
//!
//! FFmpeg отдаёт MPEG-TS в stdout. Поток режется на куски по ключевым кадрам видео
//! (флаг random_access_indicator в adaptation field), каждый кусок помнит момент начала.
//! Старые куски выбрасываются целиком, поэтому буфер всегда начинается с ключевого кадра.
//! На диск ничего не пишется до нажатия горячей клавиши (или включения записи).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

pub const TS: usize = 188;
pub const VIDEO_PID: u16 = 0x100;
pub const PMT_PID: u16 = 0x1000;

struct Chunk {
    at: Instant,
    data: Vec<u8>,
}

pub struct Recording {
    pub path: PathBuf,
    pub started: Instant,
    file: BufWriter<File>,
    waiting_key: bool,
}

pub struct Ring {
    chunks: VecDeque<Chunk>,
    bytes: usize,
    pub max_secs: f64,
    pat: Option<[u8; TS]>,
    pmt: Option<[u8; TS]>,
    partial: Vec<u8>,
    pub frames: u64,
    pub total_bytes: u64,
    pub keyframes: u64,
    rec: Option<Recording>,
    pub rec_error: Option<String>,
    /// Энкодер не выдаёт ключевые кадры (буфер сбрасывался защитой памяти).
    pub no_keyframes: bool,
}

impl Ring {
    pub fn new(max_secs: f64) -> Self {
        Ring {
            chunks: VecDeque::new(),
            bytes: 0,
            max_secs,
            pat: None,
            pmt: None,
            partial: Vec::with_capacity(TS * 512),
            frames: 0,
            total_bytes: 0,
            keyframes: 0,
            rec: None,
            rec_error: None,
            no_keyframes: false,
        }
    }

    pub fn push(&mut self, buf: &[u8]) {
        self.total_bytes += buf.len() as u64;
        self.partial.extend_from_slice(buf);
        let mut i = 0;
        let mut pkt = [0u8; TS];
        while self.partial.len() - i >= TS {
            if self.partial[i] != 0x47 {
                i += 1; // потеря синхронизации — ищем следующий 0x47
                continue;
            }
            pkt.copy_from_slice(&self.partial[i..i + TS]);
            self.on_packet(&pkt);
            i += TS;
        }
        self.partial.drain(..i);
    }

    fn on_packet(&mut self, p: &[u8; TS]) {
        let pid = (((p[1] & 0x1f) as u16) << 8) | p[2] as u16;
        let pusi = p[1] & 0x40 != 0;
        let afc = (p[3] >> 4) & 3;
        if pid == 0 {
            self.pat = Some(*p);
        } else if pid == PMT_PID && pusi {
            self.pmt = Some(*p);
        }

        if pid == VIDEO_PID && pusi {
            self.frames += 1;
            let rai = afc & 2 != 0 && p[4] > 0 && p[5] & 0x40 != 0;
            if rai {
                self.keyframes += 1;
                let now = Instant::now();
                // Завершённый кусок больше не растёт — отдаём лишнюю ёмкость.
                if let Some(last) = self.chunks.back_mut() {
                    last.data.shrink_to_fit();
                }
                let cap = self.chunks.back().map(|c| c.data.len() + c.data.len() / 4).unwrap_or(1 << 20);
                self.chunks.push_back(Chunk { at: now, data: Vec::with_capacity(cap) });
                self.trim(now);
                self.on_keyframe_rec();
            }
        }
        // Пакеты до первого ключевого кадра не нужны.
        if let Some(c) = self.chunks.back_mut() {
            c.data.extend_from_slice(p);
            self.bytes += TS;
            if pid == VIDEO_PID && pusi && self.frames % 64 == 0 {
                self.guard();
            }
        }
        if let Some(r) = &mut self.rec {
            if !r.waiting_key {
                if let Err(e) = r.file.write_all(p) {
                    self.rec_error = Some(e.to_string());
                    self.rec = None;
                }
            }
        }
    }

    /// Защита памяти: если энкодер перестал выдавать ключевые кадры, единственный кусок
    /// рос бы бесконечно. Тогда он сбрасывается, и буфер ждёт следующий ключевой кадр.
    fn guard(&mut self) {
        if self.chunks.len() == 1 {
            let age = self.chunks[0].at.elapsed().as_secs_f64();
            if age > self.max_secs * 2.0 + 10.0 {
                self.chunks.clear();
                self.bytes = 0;
                self.no_keyframes = true;
            }
        }
    }

    fn trim(&mut self, now: Instant) {
        // Держим куски, пока второй по счёту не стал старше лимита — так покрытие всегда ≥ max_secs.
        while self.chunks.len() > 2 {
            let second = self.chunks[1].at;
            if now.duration_since(second).as_secs_f64() > self.max_secs {
                let c = self.chunks.pop_front().unwrap();
                self.bytes -= c.data.len();
            } else {
                break;
            }
        }
    }

    fn headers(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(TS * 2);
        if let Some(p) = &self.pat {
            v.extend_from_slice(p);
        }
        if let Some(p) = &self.pmt {
            v.extend_from_slice(p);
        }
        v
    }

    fn on_keyframe_rec(&mut self) {
        let hdr = self.headers();
        if let Some(r) = &mut self.rec {
            if r.waiting_key {
                r.waiting_key = false;
                r.started = Instant::now();
                if let Err(e) = r.file.write_all(&hdr) {
                    self.rec_error = Some(e.to_string());
                    self.rec = None;
                }
            }
        }
    }

    /// Сколько секунд реально лежит в буфере.
    pub fn buffered_secs(&self) -> f64 {
        self.chunks.front().map(|c| c.at.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    pub fn buffered_bytes(&self) -> usize {
        self.bytes
    }

    pub fn has_keyframe(&self) -> bool {
        !self.chunks.is_empty()
    }

    /// Последние `secs` секунд как готовый TS (PAT/PMT + куски от ключевого кадра).
    pub fn snapshot(&self, secs: f64) -> Option<(Vec<u8>, f64)> {
        if self.chunks.is_empty() {
            return None;
        }
        let now = Instant::now();
        // Самый поздний кусок, начавшийся не позже, чем `secs` назад.
        let mut start = 0;
        for (i, c) in self.chunks.iter().enumerate().rev() {
            if now.duration_since(c.at).as_secs_f64() >= secs {
                start = i;
                break;
            }
        }
        let size: usize = self.chunks.iter().skip(start).map(|c| c.data.len()).sum();
        let mut out = self.headers();
        out.reserve(size);
        for c in self.chunks.iter().skip(start) {
            out.extend_from_slice(&c.data);
        }
        let dur = now.duration_since(self.chunks[start].at).as_secs_f64();
        Some((out, dur))
    }

    /// Запись начинается с текущего куска (ключевой кадр ≤ 1 с назад), чтобы момент нажатия
    /// гарантированно попал в файл. Если буфер ещё пуст — с ближайшего ключевого кадра.
    pub fn start_recording(&mut self, path: PathBuf) -> std::io::Result<()> {
        let mut file = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        let (waiting_key, started) = match self.chunks.back() {
            Some(c) => {
                file.write_all(&self.headers())?;
                file.write_all(&c.data)?;
                (false, c.at)
            }
            None => (true, Instant::now()),
        };
        self.rec = Some(Recording { path, started, file, waiting_key });
        self.rec_error = None;
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Option<(PathBuf, f64)> {
        let mut r = self.rec.take()?;
        if r.waiting_key {
            // Запись так и не началась (не было ключевого кадра) — пустой файл не нужен.
            let path = r.path.clone();
            drop(r);
            let _ = std::fs::remove_file(path);
            return None;
        }
        let _ = r.file.flush();
        Some((r.path.clone(), r.started.elapsed().as_secs_f64()))
    }

    pub fn recording_secs(&self) -> Option<f64> {
        self.rec
            .as_ref()
            .map(|r| if r.waiting_key { 0.0 } else { r.started.elapsed().as_secs_f64() })
    }
}
