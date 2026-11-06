//! Автоопределение запущенной полноэкранной игры.
//!
//! Раз в 1,5 секунды проверяется окно переднего плана: если оно закрывает весь монитор
//! (эксклюзивный fullscreen или borderless) и это не браузер / плеер / оболочка Windows,
//! оно считается игрой. Игра должна продержаться на переднем плане две проверки подряд —
//! так случайные всплывающие окна не перезапускают захват. Когда процесс игры завершается,
//! захват останавливается, но остаётся «взведённым» до следующей игры.

use super::recorder;
use super::sources::process_exe;
use crate::state::AppState;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use windows::Win32::Foundation::{CloseHandle, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    pub pid: u32,
    pub exe: String,
    pub title: String,
    #[serde(skip)]
    pub hmonitor: u64,
}

impl GameInfo {
    pub fn name(&self) -> String {
        if self.title.trim().is_empty() { self.exe.clone() } else { self.title.clone() }
    }
}

/// Программы, которые бывают на весь экран, но играми не являются.
const NOT_GAMES: &[&str] = &[
    "explorer.exe", "videonarez.exe", "applicationframehost.exe", "lockapp.exe", "searchhost.exe",
    "startmenuexperiencehost.exe", "shellexperiencehost.exe", "textinputhost.exe", "taskmgr.exe",
    "chrome.exe", "msedge.exe", "firefox.exe", "opera.exe", "browser.exe", "brave.exe", "vivaldi.exe",
    "vlc.exe", "mpc-hc64.exe", "mpc-be64.exe", "potplayermini64.exe", "wmplayer.exe", "video.ui.exe",
    "ffplay.exe", "obs64.exe", "discord.exe", "telegram.exe", "code.exe", "devenv.exe",
    "windowsterminal.exe", "powershell.exe", "cmd.exe", "mstsc.exe", "parsecd.exe", "claude.exe",
    "steamwebhelper.exe", "steam.exe", "epicgameslauncher.exe",
];

/// Полноэкранное окно переднего плана, похожее на игру.
pub fn foreground_fullscreen(exclude: &[String]) -> Option<GameInfo> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return None;
        }
        let m = mi.rcMonitor;
        if !(r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom) {
            return None;
        }
        let exe = process_exe(pid);
        let lower = exe.to_lowercase();
        if exe.is_empty()
            || NOT_GAMES.contains(&lower.as_str())
            || exclude.iter().any(|e| e.trim().eq_ignore_ascii_case(&lower))
        {
            return None;
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        Some(GameInfo {
            pid,
            exe,
            title: String::from_utf16_lossy(&buf[..n]),
            hmonitor: mon.0 as usize as u64,
        })
    }
}

pub fn alive(pid: u32) -> bool {
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = CloseHandle(h);
        ok && code == STILL_ACTIVE
    }
}

pub fn spawn_watcher(app: AppHandle) {
    std::thread::Builder::new()
        .name("game-watcher".into())
        .spawn(move || {
            let mut candidate: Option<GameInfo> = None;
            let mut last_retry = std::time::Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(1500));
                let state = app.state::<AppState>();
                let cs = state.settings().capture;
                let rec = &state.recorder;
                if cs.source != "auto" {
                    if rec.game.lock().unwrap().take().is_some() {
                        recorder::emit_status(&app);
                    }
                    candidate = None;
                    continue;
                }
                let current = rec.game.lock().unwrap().clone();

                // Игра закрылась — останавливаем захват, но ждём следующую.
                if let Some(g) = &current {
                    if !alive(g.pid) {
                        eprintln!("[auto] game exited: {}", g.exe);
                        *rec.game.lock().unwrap() = None;
                        recorder::stop_session(&app);
                        candidate = None;
                        continue;
                    }
                }
                if !rec.is_wanted() {
                    candidate = None;
                    continue;
                }

                // Игра жива, а захвата нет (FFmpeg упал при alt-tab / смене режима экрана,
                // и быстрый перезапуск не удался) — пробуем снова раз в 5 секунд.
                if current.is_some()
                    && rec.session.lock().unwrap().is_none()
                    && last_retry.elapsed() > Duration::from_secs(5)
                {
                    last_retry = std::time::Instant::now();
                    match recorder::start(&app) {
                        Ok(()) => eprintln!("[auto] capture restarted for {}", current.as_ref().unwrap().exe),
                        Err(e) => eprintln!("[auto] restart failed: {e}"),
                    }
                    continue;
                }

                let Some(fg) = foreground_fullscreen(&cs.game_exclude) else {
                    candidate = None;
                    continue;
                };
                if current.as_ref().map(|c| c.pid) == Some(fg.pid) {
                    continue;
                }
                if candidate.as_ref().map(|c| c.pid) != Some(fg.pid) {
                    candidate = Some(fg); // первая проверка — ждём подтверждения
                    continue;
                }
                // Подтверждено двумя проверками: переключаем захват на эту игру.
                candidate = None;
                eprintln!("[auto] game detected: {} (pid {})", fg.exe, fg.pid);
                *rec.game.lock().unwrap() = Some(fg);
                recorder::stop_session(&app);
                last_retry = std::time::Instant::now();
                if let Err(e) = recorder::start(&app) {
                    eprintln!("[auto] start failed: {e}");
                }
            }
        })
        .expect("spawn game watcher");
}
