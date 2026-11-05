//! Протокол clip:// — отдаёт локальный файл плееру WebView2 кусками по HTTP Range (206).
//!
//! Файл никогда не читается в память целиком: на каждый запрос читается максимум
//! MAX_CHUNK байт, поэтому перемотка по 30-гигабайтной записи мгновенная и не ест ОЗУ.

use crate::state::AppState;
use percent_encoding::percent_decode_str;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager};

const MAX_CHUNK: u64 = 4 * 1024 * 1024;

pub fn handle(app: &AppHandle, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    match serve(app, req) {
        Ok(r) => r,
        Err((code, msg)) => Response::builder()
            .status(code)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(msg.into_bytes())
            .unwrap(),
    }
}

fn mime(path: &PathBuf) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4" | "m4v" | "mov") => "video/mp4",
        Some("webm" | "mkv") => "video/webm",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

fn parse_range(s: &str, len: u64) -> Option<(u64, u64)> {
    let s = s.trim().strip_prefix("bytes=")?;
    let first = s.split(',').next()?.trim();
    let (a, b) = first.split_once('-')?;
    if a.is_empty() {
        // суффикс: последние N байт
        let n: u64 = b.parse().ok()?;
        let n = n.min(len);
        return Some((len - n, len.saturating_sub(1)));
    }
    let start: u64 = a.parse().ok()?;
    let end: u64 = if b.is_empty() { u64::MAX } else { b.parse().ok()? };
    Some((start, end))
}

fn serve(app: &AppHandle, req: &Request<Vec<u8>>) -> Result<Response<Vec<u8>>, (StatusCode, String)> {
    let raw = req.uri().path().trim_start_matches('/');
    let decoded = percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| (StatusCode::BAD_REQUEST, "bad path".to_string()))?;
    let path = PathBuf::from(decoded.as_ref());

    let state = app.state::<AppState>();
    if !state.is_allowed(&path) {
        return Err((StatusCode::FORBIDDEN, "not allowed".into()));
    }

    let mut file = File::open(&path).map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    let len = file
        .metadata()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .len();

    let (start, end) = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, len))
        .unwrap_or((0, u64::MAX));

    if len == 0 || start >= len {
        return Ok(Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{len}"))
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Vec::new())
            .unwrap());
    }

    let end = end.min(len - 1).min(start + MAX_CHUNK - 1);
    let n = (end - start + 1) as usize;
    let mut buf = vec![0u8; n];
    file.seek(SeekFrom::Start(start))
        .and_then(|_| file.read_exact(&mut buf))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, mime(&path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, n.to_string())
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::CACHE_CONTROL, "no-store")
        .body(buf)
        .unwrap())
}
