//! Трей, глобальные горячие клавиши и жизненный цикл окна.
//!
//! При закрытии окна WebView2 уничтожается целиком (а не скрывается): в фоне остаётся
//! только процесс Rust (~десятки МБ) и FFmpeg захвата. Окно пересоздаётся из трея за доли секунды.

use crate::capture::recorder;
use crate::state::AppState;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewWindowBuilder, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub struct TrayItems {
    save: MenuItem<Wry>,
    buffer: MenuItem<Wry>,
    record: MenuItem<Wry>,
}

const TRAY_ID: &str = "main";

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
                    let _ = recorder::toggle_record(&app);
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
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayItems { save, buffer, record });
    Ok(())
}

/// Обновить пункты меню и подсказку трея по текущему состоянию захвата.
pub fn refresh_tray(app: &AppHandle) {
    let st = recorder::status(app);
    let cs = app.state::<AppState>().settings().capture;
    if let Some(items) = app.try_state::<TrayItems>() {
        let armed = st.running || st.waiting_for_game;
        let _ = items.buffer.set_text(if armed { "Выключить буфер повтора" } else { "Включить буфер повтора" });
        let _ = items.save.set_enabled(st.running);
        let save_label = if cs.hotkey_save.is_empty() {
            "Сохранить повтор".to_string()
        } else {
            format!("Сохранить повтор ({})", cs.hotkey_save)
        };
        let _ = items.save.set_text(save_label);
        let _ = items.record.set_text(if st.recording_secs.is_some() { "Остановить запись" } else { "Начать запись" });
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let tip = if st.recording_secs.is_some() {
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
        };
        let _ = tray.set_tooltip(Some(tip));
    }
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
