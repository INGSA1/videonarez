use crate::config::settings::Settings;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct AppState {
    /// Файлы, которые разрешено отдавать через протокол clip:// (только открытые пользователем и их прокси).
    allowed: Mutex<HashSet<PathBuf>>,
    /// Токен фонового анализа текущего файла (миниатюры, звук, прокси). Отменяется при открытии нового.
    analysis: Mutex<CancellationToken>,
    /// Токен текущего экспорта.
    pub export: Mutex<Option<CancellationToken>>,
    pub settings: RwLock<Settings>,
    pub settings_path: Mutex<Option<PathBuf>>,
    /// Фоновый захват и буфер повтора.
    pub recorder: crate::capture::recorder::Recorder,
    /// Пакетная очередь экспорта.
    pub queue: crate::editor::queue::Queue,
}

impl AppState {
    fn key(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    pub fn allow(&self, p: &Path) {
        self.allowed.lock().unwrap().insert(Self::key(p));
    }

    pub fn is_allowed(&self, p: &Path) -> bool {
        self.allowed.lock().unwrap().contains(&Self::key(p))
    }

    /// Отменить предыдущий анализ и выдать новый токен.
    pub fn new_analysis(&self) -> CancellationToken {
        let mut g = self.analysis.lock().unwrap();
        g.cancel();
        *g = CancellationToken::new();
        g.clone()
    }

    pub fn analysis(&self) -> CancellationToken {
        self.analysis.lock().unwrap().clone()
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }
}
