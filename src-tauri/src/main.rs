// Без консольного окна в релизной сборке Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod config;
mod editor;
mod error;
mod ffmpeg;
mod shell;
mod state;
mod updates;

use error::AppResult;
use state::AppState;
use tauri::{Manager, RunEvent, State};

/// Показать файл в проводнике.
#[tauri::command]
fn reveal_in_explorer(path: String) -> AppResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.replace('/', "\\")))
            .spawn()?;
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

/// Остановить фоновый анализ (миниатюры, волна, прокси) текущего файла.
#[tauri::command]
fn cancel_analysis(state: State<'_, AppState>) {
    state.new_analysis();
}

#[tauri::command]
fn ffmpeg_version() -> AppResult<String> {
    let t = ffmpeg::tools()?;
    Ok(t.ffmpeg.to_string_lossy().into_owned())
}

/// Файл, переданный в командной строке («Открыть с помощью» в проводнике).
#[tauri::command]
fn startup_file() -> Option<String> {
    std::env::args()
        .skip(1)
        .find(|a| !a.starts_with('-') && std::path::Path::new(a).is_file())
}

/// Показать главное окно (пересоздаёт WebView2, если окно было закрыто в трей).
#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    shell::show_main(&app);
}

/// Автозапуск вместе с Windows (сразу в трей, без окна).
#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> AppResult<bool> {
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    let r = if enabled { al.enable() } else { al.disable() };
    r.map_err(|e| error::AppError::msg(format!("Не удалось изменить автозапуск: {e}")))?;
    Ok(al.is_enabled().unwrap_or(enabled))
}

#[tauri::command]
fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Запуск с `--tray` (автозапуск Windows): окно и WebView2 не создаются вовсе.
fn started_in_tray() -> bool {
    std::env::args().any(|a| a == "--tray")
}

fn main() {
    let app = tauri::Builder::default()
        // Повторный запуск (ярлык, «Открыть с помощью») показывает уже работающее окно из трея.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            shell::show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::Pending::default())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--tray"]),
        ))
        .manage(AppState::default())
        // clip://localhost/<путь> — потоковая отдача видео плееру с поддержкой Range.
        .register_asynchronous_uri_scheme_protocol("clip", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(editor::stream_server::handle(&app, &request));
            });
        })
        .setup(|app| {
            config::settings::init(app.handle())?;
            shell::setup_tray(app.handle())?;
            for e in shell::register_hotkeys(app.handle()) {
                eprintln!("hotkey: {e}");
            }
            capture::recorder::spawn_monitor(app.handle().clone());
            capture::games::spawn_watcher(app.handle().clone());
            updates::spawn_background_check(app.handle().clone());
            if let Some(w) = app.get_webview_window("main") {
                if started_in_tray() {
                    let _ = w.destroy();
                } else {
                    let _ = w.show();
                }
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                config::gpu::ensure_detected(&handle).await;
                editor::proxy::cleanup_old(&handle).await;
                if handle.state::<AppState>().settings().capture.auto_start {
                    let h = handle.clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || capture::recorder::start(&h)).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            editor::probe::probe_video_metadata,
            editor::proxy::prepare_preview,
            editor::proxy::preview_cache_size,
            editor::proxy::clear_preview_cache,
            editor::thumbnails::extract_thumbnails,
            editor::waveform::extract_waveform,
            editor::trimmer::trim_and_export,
            editor::trimmer::cancel_export,
            config::gpu::detect_encoders,
            config::settings::get_settings,
            config::settings::save_settings,
            reveal_in_explorer,
            cancel_analysis,
            ffmpeg_version,
            startup_file,
            show_main_window,
            capture::recorder::capture_sources,
            capture::recorder::capture_start,
            capture::recorder::capture_stop,
            capture::recorder::capture_status,
            capture::recorder::capture_save,
            capture::recorder::capture_toggle_record,
            capture::recorder::capture_apply_settings,
            capture::recorder::capture_recent,
            editor::tracks::extract_audio_tracks,
            editor::gallery::list_clips,
            editor::gallery::clip_preview,
            editor::gallery::open_clips_folder,
            editor::queue::queue_add,
            editor::queue::queue_list,
            editor::queue::queue_cancel,
            editor::queue::queue_clear,
            get_autostart,
            set_autostart,
            updates::check_update,
            updates::install_update,
            app_version,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Videonarez");

    app.run(|app, event| match event {
        // Последнее окно закрыто: WebView2 уже уничтожен, а приложение остаётся в трее.
        RunEvent::ExitRequested { api, code: None, .. } => {
            if app.state::<AppState>().settings().capture.close_to_tray {
                api.prevent_exit();
            }
        }
        RunEvent::Exit => capture::recorder::stop(app),
        _ => {}
    });
}
