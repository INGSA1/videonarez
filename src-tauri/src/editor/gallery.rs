//! Галерея клипов: последние видео из папки клипов и кадр-превью для каждого.
//!
//! Превью — один ключевой кадр 320 px (декодируется только он), кэшируется на диске
//! по пути + размеру + времени изменения, поэтому повторное открытие галереи бесплатно.

use crate::error::{AppError, AppResult};
use crate::ffmpeg::{ffmpeg_cmd, ffprobe_cmd, output, ts, Priority};
use crate::state::AppState;
use base64::Engine;
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Manager, State};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClipFile {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    /// Время изменения, мс от эпохи Unix.
    pub modified: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipPreview {
    pub thumb: Option<String>,
    pub duration: f64,
}

const EXT: &[&str] = &["mp4", "mkv", "mov", "webm"];

/// Папки раздела галереи: "recordings" — целые записи; иначе клипы. Старые файлы, лежащие
/// прямо в общей папке (до разделения на подпапки), показываются среди клипов.
fn dirs_for(state: &AppState, kind: &str) -> Vec<PathBuf> {
    let s = state.settings();
    if kind == "recordings" {
        vec![s.recordings_dir()]
    } else {
        vec![s.clips_dir(), PathBuf::from(&s.output_dir)]
    }
}

/// Видеофайлы в папках (без вложенных), новые сначала.
fn scan(dirs: &[PathBuf]) -> Vec<ClipFile> {
    let mut v: Vec<ClipFile> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten())
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_string_lossy().to_lowercase();
            let name = p.file_name()?.to_string_lossy().into_owned();
            // скрытые/временные файлы записи не показываем
            if !EXT.contains(&ext.as_str()) || name.starts_with('.') {
                return None;
            }
            let md = e.metadata().ok()?;
            let modified = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
            Some(ClipFile { path: p.to_string_lossy().into_owned(), name, size_bytes: md.len(), modified })
        })
        .collect();
    v.sort_by(|a, b| b.modified.cmp(&a.modified));
    v
}

#[tauri::command]
pub fn list_clips(state: State<'_, AppState>, limit: usize, kind: Option<String>) -> AppResult<Vec<ClipFile>> {
    let mut v = scan(&dirs_for(&state, kind.as_deref().unwrap_or("clips")));
    v.truncate(limit.clamp(1, 1000));
    Ok(v)
}

// ---------- Переименование и удаление ----------

/// Все папки библиотеки: клипы, записи и общая папка (старые файлы).
fn library_dirs(state: &AppState) -> Vec<PathBuf> {
    let s = state.settings();
    vec![s.clips_dir(), s.recordings_dir(), PathBuf::from(&s.output_dir)]
}

/// Команды галереи меняют только видеофайлы, лежащие прямо в папках Videonarez.
fn check_in_library(state: &AppState, p: &Path) -> AppResult<()> {
    let parent = p.parent().and_then(|d| d.canonicalize().ok());
    let ok = p.is_file()
        && p.extension().map_or(false, |e| EXT.contains(&e.to_string_lossy().to_lowercase().as_str()))
        && library_dirs(state).iter().any(|d| d.canonicalize().ok() == parent);
    if ok {
        Ok(())
    } else {
        Err(AppError::msg("Этот файл не из папки клипов Videonarez"))
    }
}

/// Имя файла без запрещённых в Windows символов.
fn sanitize_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control() && !"\\/:*?\"<>|".contains(*c)).collect();
    s.trim().trim_end_matches('.').trim().chars().take(120).collect()
}

/// Переименовать клип (расширение сохраняется). Возвращает новый путь.
#[tauri::command]
pub fn rename_clip(state: State<'_, AppState>, path: String, name: String) -> AppResult<String> {
    let src = PathBuf::from(&path);
    check_in_library(&state, &src)?;
    let stem = sanitize_name(&name);
    if stem.is_empty() {
        return Err(AppError::msg("Название не может быть пустым"));
    }
    let ext = src.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let dst = src.with_file_name(format!("{stem}.{ext}"));
    if dst == src {
        return Ok(path);
    }
    // Смена только регистра букв — тот же файл, это не конфликт.
    let same = dst.to_string_lossy().to_lowercase() == src.to_string_lossy().to_lowercase();
    if dst.exists() && !same {
        return Err(AppError::msg("Клип с таким названием уже есть"));
    }
    std::fs::rename(&src, &dst).map_err(|e| AppError::msg(format!("Не удалось переименовать: {e}")))?;
    Ok(dst.to_string_lossy().into_owned())
}

/// Отправить файлы в корзину Windows (их можно восстановить).
fn recycle(paths: &[PathBuf]) -> AppResult<()> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    if paths.is_empty() {
        return Ok(());
    }
    // Список путей: каждый завершён нулём, в конце ещё один ноль.
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.to_string_lossy().replace('/', "\\").encode_utf16());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc == 0 && !op.fAnyOperationsAborted.as_bool() {
        Ok(())
    } else {
        Err(AppError::msg(format!("Не удалось отправить в корзину (код {rc})")))
    }
}

/// Удалить клип в корзину.
#[tauri::command]
pub fn delete_clip(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let p = PathBuf::from(&path);
    check_in_library(&state, &p)?;
    recycle(&[p])
}

// ---------- Автоочистка ----------

/// Имя с отметкой времени записи (`…_2026-11-07_23-40-48…`) — файл никто не переименовывал.
/// Переименованные клипы автоочистка не трогает: раз дали название, значит клип нужен.
fn is_auto_named(name: &str) -> bool {
    let b = name.as_bytes();
    const PAT: &[u8] = b"_dddd-dd-dd_dd-dd-dd";
    b.windows(PAT.len()).any(|w| {
        w.iter().zip(PAT).all(|(c, p)| if *p == b'd' { c.is_ascii_digit() } else { c == p })
    })
}

/// Какие файлы убрать: старше `days` дней, затем самые старые, пока библиотека больше `max_bytes`.
/// 0 — правило выключено. `files` — вся библиотека; переименованные и только что записанные
/// (младше 2 минут) не удаляются, но в общий объём входят.
fn pick_for_cleanup(files: &[ClipFile], now_ms: u64, days: u32, max_bytes: u64) -> Vec<usize> {
    let removable = |f: &ClipFile| is_auto_named(&f.name) && now_ms.saturating_sub(f.modified) > 120_000;
    let mut picked = vec![false; files.len()];
    if days > 0 {
        let limit = days as u64 * 86_400_000;
        for (i, f) in files.iter().enumerate() {
            picked[i] = removable(f) && now_ms.saturating_sub(f.modified) > limit;
        }
    }
    if max_bytes > 0 {
        let mut total: u64 = files.iter().zip(&picked).filter(|(_, p)| !**p).map(|(f, _)| f.size_bytes).sum();
        let mut order: Vec<usize> = (0..files.len()).filter(|&i| !picked[i] && removable(&files[i])).collect();
        order.sort_by_key(|&i| files[i].modified); // самые старые первыми
        for i in order {
            if total <= max_bytes {
                break;
            }
            picked[i] = true;
            total -= files[i].size_bytes;
        }
    }
    (0..files.len()).filter(|&i| picked[i]).collect()
}

#[derive(Serialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub files: usize,
    pub bytes: u64,
}

/// Применить правила автоочистки из настроек. Файлы уходят в корзину.
pub fn cleanup(state: &AppState) -> AppResult<CleanupResult> {
    let s = state.settings();
    if s.cleanup_days == 0 && s.cleanup_max_gb == 0 {
        return Ok(CleanupResult::default());
    }
    let files = scan(&library_dirs(state));
    let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let idx = pick_for_cleanup(&files, now, s.cleanup_days, s.cleanup_max_gb as u64 * (1 << 30));
    let paths: Vec<PathBuf> = idx.iter().map(|&i| PathBuf::from(&files[i].path)).collect();
    recycle(&paths)?;
    Ok(CleanupResult { files: idx.len(), bytes: idx.iter().map(|&i| files[i].size_bytes).sum() })
}

/// «Очистить сейчас» из настроек.
#[tauri::command]
pub fn run_cleanup(app: AppHandle, state: State<'_, AppState>) -> AppResult<CleanupResult> {
    let r = cleanup(&state)?;
    if r.files > 0 {
        let _ = tauri::Emitter::emit(&app, "clips-changed", ());
    }
    Ok(r)
}

/// Фоновая автоочистка: через минуту после запуска и затем раз в полчаса.
pub fn spawn_cleanup(app: AppHandle) {
    let _ = std::thread::Builder::new().name("clip-cleanup".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        loop {
            match cleanup(&app.state::<AppState>()) {
                Ok(r) if r.files > 0 => {
                    let _ = tauri::Emitter::emit(&app, "clips-changed", ());
                }
                Err(e) => eprintln!("[cleanup] {e}"),
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_secs(1800));
        }
    });
}

fn cache_path(app: &AppHandle, path: &Path) -> AppResult<PathBuf> {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        m.len().hash(&mut h);
        if let Ok(t) = m.modified() {
            t.hash(&mut h);
        }
    }
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::msg(e.to_string()))?
        .join("thumbs");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{:016x}.jpg", h.finish())))
}

async fn duration(path: &str) -> f64 {
    let Ok(mut cmd) = ffprobe_cmd() else { return 0.0 };
    cmd.args(["-show_entries", "format=duration", "-of", "csv=p=0"]).arg(path);
    output(cmd, None)
        .await
        .ok()
        .and_then(|b| String::from_utf8_lossy(&b).trim().parse().ok())
        .unwrap_or(0.0)
}

#[tauri::command]
pub async fn clip_preview(app: AppHandle, path: String) -> AppResult<ClipPreview> {
    let src = PathBuf::from(&path);
    if !src.is_file() {
        return Err(AppError::msg("Файл не найден"));
    }
    let dur = duration(&path).await;
    let cache = cache_path(&app, &src)?;
    if !cache.is_file() {
        // Кадр примерно с трети клипа — обычно уже «самое интересное», но не самый конец.
        // Если после этой точки нет ключевых кадров (бывает у записей с редкими IDR),
        // берётся первый ключевой кадр файла.
        let at = if dur > 0.0 { (dur * 0.33).min(dur - 0.1).max(0.0) } else { 0.0 };
        for seek in [at, 0.0] {
            let mut cmd = ffmpeg_cmd(Priority::BelowNormal, false)?;
            cmd.args(["-noaccurate_seek", "-skip_frame", "nokey", "-ss", &ts(seek), "-i", &path]);
            cmd.args([
                "-map", "0:v:0", "-an", "-frames:v", "1",
                "-vf", "scale=320:-2:flags=bicubic,format=yuvj420p",
                "-q:v", "4", "-f", "image2",
            ]);
            cmd.arg(&cache);
            let _ = output(cmd, None).await;
            if cache.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                break;
            }
        }
        if !cache.is_file() {
            return Ok(ClipPreview { thumb: None, duration: dur });
        }
    }
    let thumb = std::fs::read(&cache).ok().map(|b| {
        format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b))
    });
    Ok(ClipPreview { thumb, duration: dur })
}

/// Открыть папку клипов в проводнике.
#[tauri::command]
pub fn open_clips_folder(state: State<'_, AppState>, kind: Option<String>) -> AppResult<()> {
    let dir = dirs_for(&state, kind.as_deref().unwrap_or("clips")).remove(0);
    std::fs::create_dir_all(&dir)?;
    std::process::Command::new("explorer").arg(&dir).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, age_days: u64, gb: u64) -> ClipFile {
        ClipFile { path: name.into(), name: name.into(), size_bytes: gb << 30, modified: NOW - age_days * 86_400_000 }
    }
    const NOW: u64 = 2_000_000_000_000;

    #[test]
    fn auto_names_and_sanitize() {
        assert!(is_auto_named("Replay_2026-11-07_23-40-48.mp4"));
        assert!(is_auto_named("cs2_2026-11-07_23-40-48_01.mp4"));
        assert!(!is_auto_named("Эйс на мираже.mp4"));
        assert!(!is_auto_named("Replay_2026-11-07.mp4"));
        assert_eq!(sanitize_name("  эйс: 1/5 <топ>.  "), "эйс 15 топ");
        assert_eq!(sanitize_name("..."), "");
    }

    #[test]
    fn cleanup_by_age_keeps_renamed_and_fresh() {
        let files = vec![
            f("Replay_2026-01-01_10-00-00.mp4", 40, 1),
            f("Лучший момент.mp4", 40, 1),
            f("Replay_2026-02-01_10-00-00.mp4", 5, 1),
        ];
        assert_eq!(pick_for_cleanup(&files, NOW, 30, 0), vec![0]);
        assert!(pick_for_cleanup(&files, NOW, 0, 0).is_empty());
    }

    #[test]
    fn cleanup_by_size_removes_oldest_first() {
        let files = vec![
            f("Replay_2026-03-03_10-00-00.mp4", 1, 4),
            f("Replay_2026-03-01_10-00-00.mp4", 3, 4),
            f("Важный.mp4", 9, 4),
            f("Replay_2026-03-02_10-00-00.mp4", 2, 4),
        ];
        // 16 ГБ при лимите 10: уходят два самых старых автоименованных, переименованный остаётся.
        assert_eq!(pick_for_cleanup(&files, NOW, 0, 10 << 30), vec![1, 3]);
        // Только что записанный файл не трогаем, даже если лимит превышен.
        let fresh = vec![ClipFile { modified: NOW - 1000, ..f("Replay_2026-03-03_10-00-00.mp4", 0, 50) }];
        assert!(pick_for_cleanup(&fresh, NOW, 1, 1 << 30).is_empty());
    }

    /// Настоящая корзина: файл исчезает из папки. Запуск: cargo test -- --ignored recycle_real
    #[test]
    #[ignore]
    fn recycle_real() {
        let p = std::env::temp_dir().join("videonarez_recycle_test.mp4");
        std::fs::write(&p, b"test").unwrap();
        recycle(&[p.clone()]).unwrap();
        assert!(!p.exists());
    }
}
