//! Поиск и запуск sidecar-бинарников FFmpeg / FFprobe.
//!
//! Все процессы запускаются без консольного окна и с пониженным приоритетом
//! (BELOW_NORMAL / IDLE), чтобы планировщик Windows отдавал CPU играм и UI.

use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    Idle,
    #[default]
    BelowNormal,
    Normal,
}

pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

static TOOLS: OnceLock<Tools> = OnceLock::new();

fn locate(name: &str) -> Option<PathBuf> {
    let exe = format!("{name}.exe");
    // 1) Рядом с exe приложения (так Tauri кладёт externalBin в установленной сборке и в dev).
    if let Ok(cur) = std::env::current_exe() {
        if let Some(dir) = cur.parent() {
            let p = dir.join(&exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // 2) Исходная папка sidecar-бинарников (dev-режим).
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!("{name}-x86_64-pc-windows-msvc.exe"));
    if dev.is_file() {
        return Some(dev);
    }
    // 3) PATH.
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(&exe))
            .find(|p| p.is_file())
    })
}

pub fn tools() -> AppResult<&'static Tools> {
    if let Some(t) = TOOLS.get() {
        return Ok(t);
    }
    let ffmpeg = locate("ffmpeg").ok_or_else(|| {
        AppError::msg("Не найден ffmpeg.exe (ожидается рядом с приложением или в PATH)")
    })?;
    let ffprobe = locate("ffprobe").ok_or_else(|| {
        AppError::msg("Не найден ffprobe.exe (ожидается рядом с приложением или в PATH)")
    })?;
    Ok(TOOLS.get_or_init(|| Tools { ffmpeg, ffprobe }))
}

fn base_command(bin: &Path, priority: Priority) -> Command {
    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let class = match priority {
            Priority::Idle => 0x0000_0040,        // IDLE_PRIORITY_CLASS
            Priority::BelowNormal => 0x0000_4000, // BELOW_NORMAL_PRIORITY_CLASS
            Priority::Normal => 0x0000_0020,      // NORMAL_PRIORITY_CLASS
        };
        cmd.creation_flags(CREATE_NO_WINDOW | class);
    }
    #[cfg(not(windows))]
    let _ = priority;
    cmd
}

/// ffmpeg с общими флагами. `progress` добавляет машиночитаемый прогресс в stdout
/// (глобальные опции обязаны идти до входов/выходов).
pub fn ffmpeg_cmd(priority: Priority, progress: bool) -> AppResult<Command> {
    let mut c = base_command(&tools()?.ffmpeg, priority);
    c.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);
    if progress {
        c.args(["-progress", "pipe:1", "-nostats"]);
    }
    Ok(c)
}

pub fn ffprobe_cmd() -> AppResult<Command> {
    let mut c = base_command(&tools()?.ffprobe, Priority::Normal);
    c.args(["-v", "error"]);
    Ok(c)
}

pub fn tail(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    let s = s.trim();
    if s.is_empty() {
        return "FFmpeg завершился с ошибкой".into();
    }
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(700);
    chars[start..].iter().collect()
}

/// Запустить и собрать stdout целиком (для коротких операций: probe, миниатюры).
pub async fn output(mut cmd: Command, cancel: Option<&CancellationToken>) -> AppResult<Vec<u8>> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn()?;
    let fut = child.wait_with_output();
    let out = match cancel {
        // При отмене future дропается вместе с процессом (kill_on_drop).
        Some(c) => tokio::select! {
            r = fut => r?,
            _ = c.cancelled() => return Err(AppError::Cancelled),
        },
        None => fut.await?,
    };
    if !out.status.success() {
        return Err(AppError::Msg(tail(&out.stderr)));
    }
    Ok(out.stdout)
}

/// Запустить ffmpeg (созданный с `progress = true`) и сообщать текущую позицию в секундах.
pub async fn run_progress(
    mut cmd: Command,
    cancel: &CancellationToken,
    mut on_time: impl FnMut(f64),
) -> AppResult<()> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let mut lines = BufReader::new(stdout).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line? {
                Some(l) => {
                    if let Some(v) = l.strip_prefix("out_time_us=") {
                        if let Ok(us) = v.trim().parse::<i64>() {
                            if us >= 0 { on_time(us as f64 / 1_000_000.0); }
                        }
                    }
                }
                None => break,
            },
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return Err(AppError::Cancelled);
            }
        }
    }
    let status = tokio::select! {
        s = child.wait() => s?,
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(AppError::Cancelled);
        }
    };
    let err = err_task.await.unwrap_or_default();
    if !status.success() {
        return Err(AppError::Msg(tail(&err)));
    }
    Ok(())
}

pub fn ts(t: f64) -> String {
    format!("{:.6}", t.max(0.0))
}
