//! Сигнал сохранения проигрывается и при закрытом окне.

use crate::error::{AppError, AppResult};
use std::borrow::Cow;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_NOSTOP};

const BUILTIN: &[u8] = include_bytes!("../../sounds/clip_saved.wav");
const SAMPLE_RATE: u32 = 24_000;
const MAX_SECONDS: &str = "5";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct CustomSound {
    path: String,
    modified: Option<SystemTime>,
    wav: Arc<[u8]>,
}

static CUSTOM: Mutex<Option<CustomSound>> = Mutex::new(None);
static PLAYING: Mutex<()> = Mutex::new(());

pub fn prepare(path: &str) -> AppResult<()> {
    if !path.is_empty() {
        custom_sound(path)?;
    }
    Ok(())
}

fn custom_sound(path: &str) -> AppResult<Arc<[u8]>> {
    let info = std::fs::metadata(path)
        .map_err(|_| AppError::msg("Файл сигнала недоступен. Выберите его заново."))?;
    if !info.is_file() {
        return Err(AppError::msg("Выберите аудиофайл для сигнала"));
    }
    let modified = info.modified().ok();
    if let Some(sound) = CUSTOM.lock().unwrap().as_ref() {
        if sound.path == path && sound.modified == modified {
            return Ok(sound.wav.clone());
        }
    }

    let ffmpeg = &crate::ffmpeg::tools()?.ffmpeg;
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-protocol_whitelist", "file,pipe", "-i"])
        .arg(Path::new(path))
        .args(["-map", "0:a:0", "-t", MAX_SECONDS, "-ac", "1", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .args(["-f", "s16le", "-c:a", "pcm_s16le", "pipe:1"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(AppError::msg("Не удалось прочитать звук из файла. Выберите другой аудиофайл."));
    }
    let wav: Arc<[u8]> = pcm_wav(&output.stdout).into();
    *CUSTOM.lock().unwrap() = Some(CustomSound { path: path.into(), modified, wav: wav.clone() });
    Ok(wav)
}

fn pcm_wav(samples: &[u8]) -> Vec<u8> {
    let size = samples.len() as u32;
    let mut wav = Vec::with_capacity(44 + samples.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(size + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // Моно
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&size.to_le_bytes());
    wav.extend_from_slice(samples);
    wav
}

fn at_volume(wav: &[u8], volume: u8) -> AppResult<Cow<'_, [u8]>> {
    if volume >= 100 {
        return Ok(Cow::Borrowed(wav));
    }
    let mut scaled = wav.to_vec();
    let mut offset = 12;
    while let Some(header) = wav.get(offset..offset + 8) {
        let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let Some(chunk) = scaled.get_mut(start..start.saturating_add(size)) else { break };
        if &header[..4] == b"data" {
            for bytes in chunk.chunks_exact_mut(2) {
                let sample = i16::from_le_bytes([bytes[0], bytes[1]]);
                let adjusted = i32::from(sample) * i32::from(volume) / 100;
                bytes.copy_from_slice(&(adjusted as i16).to_le_bytes());
            }
            return Ok(Cow::Owned(scaled));
        }
        offset = start + size + (size % 2);
    }
    Err(AppError::msg("Данные сигнала повреждены"))
}

fn play(wav: &[u8], volume: u8) -> AppResult<()> {
    let wav = at_volume(wav, volume)?;
    // Синхронный вызов держит буфер живым. Проигрывается в фоновом потоке.
    let ok = unsafe {
        PlaySoundW(PCWSTR(wav.as_ptr().cast()), None, SND_MEMORY | SND_NODEFAULT | SND_NOSTOP)
    };
    if ok.as_bool() { Ok(()) } else { Err(AppError::msg("Не удалось воспроизвести сигнал")) }
}

pub fn notify(path: String, volume: u8) {
    if volume == 0 {
        return;
    }
    std::thread::spawn(move || {
        let Ok(_guard) = PLAYING.try_lock() else { return };
        let custom = if path.is_empty() { None } else { custom_sound(&path).ok() };
        let _ = play(custom.as_deref().unwrap_or(BUILTIN), volume);
    });
}

#[tauri::command]
pub async fn preview_capture_sound(path: String, volume: u8) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = PLAYING.try_lock().map_err(|_| AppError::msg("Сигнал уже воспроизводится"))?;
        let custom = if path.is_empty() { None } else { Some(custom_sound(&path)?) };
        if volume > 0 {
            play(custom.as_deref().unwrap_or(BUILTIN), volume)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::{at_volume, pcm_wav, BUILTIN};

    #[test]
    fn volume_scales_pcm_without_changing_header() {
        let input = [20_000_i16, -30_000].map(i16::to_le_bytes).concat();
        let wav = pcm_wav(&input);
        let half = at_volume(&wav, 50).unwrap();
        assert_eq!(&half[..44], &wav[..44]);
        let samples = [10_000_i16, -15_000].map(i16::to_le_bytes).concat();
        assert_eq!(&half[44..], samples);
        assert_eq!(&at_volume(&wav, 0).unwrap()[44..], &[0; 4]);
        assert_eq!(at_volume(&wav, 100).unwrap().as_ref(), wav);
    }

    #[test]
    fn built_in_sound_can_be_reduced() {
        let half = at_volume(BUILTIN, 50).unwrap();
        assert_eq!(half.len(), BUILTIN.len());
        assert_ne!(half.as_ref(), BUILTIN);
    }
}
