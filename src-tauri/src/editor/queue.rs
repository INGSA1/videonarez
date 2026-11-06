//! Пакетная очередь экспорта: задания выполняются строго по одному в фоне,
//! пока пользователь открывает и размечает следующие файлы.

use crate::editor::trimmer::{export, TrimMode, TrimRequest};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: u64,
    pub label: String,
    pub input: String,
    pub segments: usize,
    pub mode: String,
    /// queued | running | done | failed | cancelled
    pub status: String,
    pub files: Vec<String>,
    pub notes: Vec<String>,
    pub error: Option<String>,
    pub elapsed_sec: f64,
}

struct Entry {
    job: Job,
    req: TrimRequest,
}

#[derive(Default)]
pub struct Queue {
    entries: Mutex<Vec<Entry>>,
    next_id: AtomicU64,
    worker: AtomicBool,
}

impl Queue {
    fn snapshot(&self) -> Vec<Job> {
        self.entries.lock().unwrap().iter().map(|e| e.job.clone()).collect()
    }
}

fn emit(app: &AppHandle) {
    let _ = app.emit("queue-update", app.state::<AppState>().queue.snapshot());
}

fn mode_name(m: TrimMode) -> &'static str {
    match m {
        TrimMode::Copy => "copy",
        TrimMode::Smart => "smart",
        TrimMode::Encode => "encode",
    }
}

#[tauri::command]
pub fn queue_add(app: AppHandle, state: State<'_, AppState>, req: TrimRequest, label: String) -> AppResult<u64> {
    if req.segments.is_empty() {
        return Err(AppError::msg("Нет фрагментов для экспорта"));
    }
    if let Some(s) = req.segments.iter().find(|s| s.end - s.start < 0.05) {
        return Err(AppError::msg(format!("Слишком короткий фрагмент: {:.2}–{:.2} с", s.start, s.end)));
    }
    let id = state.queue.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let job = Job {
        id,
        label,
        input: req.input.clone(),
        segments: req.segments.len(),
        mode: mode_name(req.mode).into(),
        status: "queued".into(),
        files: Vec::new(),
        notes: Vec::new(),
        error: None,
        elapsed_sec: 0.0,
    };
    state.queue.entries.lock().unwrap().push(Entry { job, req });
    emit(&app);
    ensure_worker(&app);
    Ok(id)
}

fn ensure_worker(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.queue.worker.swap(true, Ordering::SeqCst) {
        return; // уже работает
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<AppState>();
            // Следующее задание из очереди.
            let next = {
                let mut list = state.queue.entries.lock().unwrap();
                list.iter_mut().find(|e| e.job.status == "queued").map(|e| {
                    e.job.status = "running".into();
                    (e.job.id, e.req.clone())
                })
            };
            let Some((id, req)) = next else { break };
            emit(&app);

            let token = CancellationToken::new();
            *state.export.lock().unwrap() = Some(token.clone());
            let started = Instant::now();
            let res = export(&app, &state, &req, token).await;
            *state.export.lock().unwrap() = None;

            {
                let mut list = state.queue.entries.lock().unwrap();
                if let Some(e) = list.iter_mut().find(|e| e.job.id == id) {
                    e.job.elapsed_sec = started.elapsed().as_secs_f64();
                    match res {
                        Ok((files, notes)) => {
                            e.job.status = "done".into();
                            e.job.files = files.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
                            e.job.notes = notes;
                        }
                        Err(AppError::Cancelled) => e.job.status = "cancelled".into(),
                        Err(err) => {
                            e.job.status = "failed".into();
                            e.job.error = Some(err.to_string());
                        }
                    }
                }
            }
            emit(&app);
        }
        state_reset(&app);
    });
}

fn state_reset(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.queue.worker.store(false, Ordering::SeqCst);
    // Задание могло прийти, пока поток завершался.
    let pending = state.queue.entries.lock().unwrap().iter().any(|e| e.job.status == "queued");
    if pending {
        ensure_worker(app);
    }
}

#[tauri::command]
pub fn queue_list(state: State<'_, AppState>) -> Vec<Job> {
    state.queue.snapshot()
}

#[tauri::command]
pub fn queue_cancel(app: AppHandle, state: State<'_, AppState>, id: u64) {
    let running = {
        let mut list = state.queue.entries.lock().unwrap();
        match list.iter_mut().find(|e| e.job.id == id) {
            Some(e) if e.job.status == "queued" => {
                e.job.status = "cancelled".into();
                false
            }
            Some(e) => e.job.status == "running",
            None => false,
        }
    };
    if running {
        if let Some(t) = state.export.lock().unwrap().as_ref() {
            t.cancel();
        }
    }
    emit(&app);
}

/// Убрать из списка завершённые / отменённые / упавшие задания.
#[tauri::command]
pub fn queue_clear(app: AppHandle, state: State<'_, AppState>) {
    state
        .queue
        .entries
        .lock()
        .unwrap()
        .retain(|e| e.job.status == "queued" || e.job.status == "running");
    emit(&app);
}
