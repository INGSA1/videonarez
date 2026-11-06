// Без консольного окна в релизной сборке Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod config;
mod editor;
mod error;
mod ffmpeg;
mod shell;
mod state;

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

fn main() {
    let app = tauri::Builder::default()
        // Повторный запуск (ярлык, «Открыть с помощью») показывает уже работающее окно из трея.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            shell::show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
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
