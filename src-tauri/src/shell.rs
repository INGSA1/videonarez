//! Трей, глобальные горячие клавиши и жизненный цикл окна.
//!
//! При закрытии окна WebView2 уничтожается целиком (а не скрывается): в фоне остаётся
//! только процесс Rust (~десятки МБ) и FFmpeg захвата. Окно пересоздаётся из трея за доли секунды.

use crate::capture::recorder;
use crate::state::AppState;
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewWindowBuilder, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub struct TrayItems {
    save: MenuItem<Wry>,
    buffer: MenuItem<Wry>,
    record: MenuItem<Wry>,
    idle_icon: Option<Image<'static>>,
    recording_icon: Option<Image<'static>>,
    display: Mutex<Option<TrayDisplay>>,
}

#[derive(Clone, PartialEq, Eq)]
struct TrayDisplay {
    armed: bool,
    running: bool,
    recording: bool,
    save_label: String,
    tooltip: String,
}

const TRAY_ID: &str = "main";

fn recording_icon(icon: &Image<'_>) -> Image<'static> {
    let (width, height) = (icon.width(), icon.height());
    let mut pixels = icon.rgba().to_vec();
    let radius = width.min(height) as f32 * 0.27;
    let center_x = width as f32 - radius;
    let center_y = radius;
    for (i, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        let dx = (i % width as usize) as f32 + 0.5 - center_x;
        let dy = (i / width as usize) as f32 + 0.5 - center_y;
        let distance = dx.hypot(dy);
        if distance <= radius {
            pixel.copy_from_slice(if distance < radius * 0.78 {
                &[245, 43, 54, 255]
            } else {
                &[23, 23, 26, 255]
            });
        }
    }
    Image::new_owned(pixels, width, height)
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    if let Some(cfg) = app.config().app.windows.first() {
        match WebviewWindowBuilder::from_config(app, cfg).and_then(|b| b.build()) {
            Ok(w) => {
                // В конфиге окно скрыто (чтобы запуск в трей не мигал окном) — показываем явно.
                let _ = w.show();
                let _ = w.set_focus();
            }
            Err(e) => eprintln!("cannot create window: {e}"),
        }
    }
}

pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть Videonarez", true, None::<&str>)?;
    let save = MenuItem::with_id(app, "save", "Сохранить повтор", false, None::<&str>)?;
    let buffer = MenuItem::with_id(app, "buffer", "Включить буфер повтора", true, None::<&str>)?;
    let record = MenuItem::with_id(app, "record", "Начать запись", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &sep1, &save, &buffer, &record, &sep2, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Videonarez")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => show_main(app),
            "save" => {
                let _ = recorder::save_replay(app, None);
            }
            "buffer" => {
                let app = app.clone();
                std::thread::spawn(move || {
                    let armed = app.state::<AppState>().recorder.is_wanted();
                    if armed {
                        recorder::stop(&app);
                    } else if let Err(e) = recorder::start(&app) {
                        let _ = app.emit("capture-error", e.to_string());
                    }
                });
            }
            "record" => {
                let app = app.clone();
                std::thread::spawn(move || {
                    if let Err(e) = recorder::toggle_record(&app) {
                        let _ = app.emit("capture-error", e.to_string());
                    }
                });
            }
            "quit" => {
                recorder::stop(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(tray.app_handle());
            }
        });
    let idle_icon = app.default_window_icon().cloned().map(Image::to_owned);
    let recording_icon = idle_icon.as_ref().map(recording_icon);
    if let Some(icon) = &idle_icon {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayItems { save, buffer, record, idle_icon, recording_icon, display: Mutex::new(None) });
    Ok(())
}

/// Обновить значок, подсказку и меню только при изменении отображаемого состояния.
pub fn refresh_tray(app: &AppHandle, st: &recorder::CaptureStatus) {
    let Some(items) = app.try_state::<TrayItems>() else { return };
    let cs = app.state::<AppState>().settings().capture;
    let recording = st.recording_secs.is_some();
    let display = TrayDisplay {
        armed: st.running || st.waiting_for_game,
        running: st.running,
        recording,
        save_label: if cs.hotkey_save.is_empty() {
            "Сохранить повтор".to_string()
        } else {
            format!("Сохранить повтор ({})", cs.hotkey_save)
        },
        tooltip: if recording {
            "Videonarez — идёт запись".to_string()
        } else if st.running {
            match &st.game {
                Some(g) => format!("Videonarez — буфер {} с · {g}", st.max_secs as u32),
                None => format!("Videonarez — буфер повтора {} с", st.max_secs as u32),
            }
        } else if st.waiting_for_game {
            "Videonarez — ожидание игры".to_string()
        } else {
            "Videonarez".to_string()
        },
    };
    {
        let mut last = items.display.lock().unwrap();
        if last.as_ref() == Some(&display) {
            return;
        }
        *last = Some(display.clone());
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let items = handle.state::<TrayItems>();
        if items.display.lock().unwrap().as_ref() != Some(&display) {
            return;
        }
        let _ = items.buffer.set_text(if display.armed { "Выключить буфер повтора" } else { "Включить буфер повтора" });
        let _ = items.save.set_enabled(display.running);
        let _ = items.save.set_text(display.save_label);
        let _ = items.record.set_text(if display.recording { "Остановить запись" } else { "Начать запись" });
        if let Some(tray) = handle.tray_by_id(TRAY_ID) {
            let icon = if display.recording { &items.recording_icon } else { &items.idle_icon };
            let _ = tray.set_icon(icon.clone());
            let _ = tray.set_tooltip(Some(display.tooltip));
        }
    });
}

/// (Пере)регистрация глобальных горячих клавиш. Возвращает ошибки (занятые/неверные сочетания).
pub fn register_hotkeys(app: &AppHandle) -> Vec<String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let cs = app.state::<AppState>().settings().capture;
    let mut errors = Vec::new();
    if !cs.hotkey_save.trim().is_empty() {
        if let Err(e) = gs.on_shortcut(cs.hotkey_save.trim(), |app, _s, ev| {
            if ev.state == ShortcutState::Pressed {
                if let Err(e) = recorder::save_replay(app, None) {
                    let _ = app.emit("capture-error", e.to_string());
                }
            }
        }) {
            errors.push(format!("{}: {e}", cs.hotkey_save));
        }
    }
    if !cs.hotkey_record.trim().is_empty() && cs.hotkey_record.trim() != cs.hotkey_save.trim() {
        if let Err(e) = gs.on_shortcut(cs.hotkey_record.trim(), |app, _s, ev| {
            if ev.state == ShortcutState::Pressed {
                let app = app.clone();
                std::thread::spawn(move || {
                    if let Err(e) = recorder::toggle_record(&app) {
                        let _ = app.emit("capture-error", e.to_string());
                    }
                });
            }
        }) {
            errors.push(format!("{}: {e}", cs.hotkey_record));
        }
    }
    errors
}
