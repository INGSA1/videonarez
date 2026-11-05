//! Движок нарезки: мгновенный Copy, умная склейка (Smart Cut) и перекодирование на GPU.

use crate::config::gpu::{self, EncoderInfo};
use crate::config::settings::Settings;
use crate::editor::encode::{container_args, pix_fmt, video_args, Rate};
use crate::editor::probe::{self, VideoMeta};
use crate::editor::proxy::tmp_root;
use crate::error::{AppError, AppResult};
use crate::ffmpeg::{ffmpeg_cmd, ffprobe_cmd, output, run_progress, ts, Priority};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

// ---------- Запрос из UI ----------

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum TrimMode {
    Copy,
    Smart,
    Encode,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum RateKind {
    Quality,
    Bitrate,
    Size,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub name: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackSel {
    pub index: usize,
    pub volume: f32,
    pub enabled: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EncodeSettings {
    /// auto | nvenc | amf | qsv | cpu
    pub encoder: String,
    /// h264 | hevc | av1
    pub codec: String,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub crop: Option<CropRect>,
    pub rate: RateKind,
    pub quality: u32,
    pub bitrate_kbps: u32,
    pub target_mb: f64,
    pub audio_tracks: Vec<AudioTrackSel>,
    pub audio_bitrate_kbps: u32,
    pub mute: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrimRequest {
    pub input: String,
    pub segments: Vec<Segment>,
    pub merge: bool,
    pub mode: TrimMode,
    /// mp4 | mkv
    pub container: String,
    pub output_dir: String,
    pub base_name: String,
    pub encode: EncodeSettings,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    percent: f64,
    stage: String,
    current: usize,
    total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub files: Vec<String>,
    pub notes: Vec<String>,
    pub elapsed_sec: f64,
}

// ---------- Контекст выполнения ----------

struct Ctx<'a> {
    app: &'a AppHandle,
    req: &'a TrimRequest,
    meta: VideoMeta,
    settings: Settings,
    token: CancellationToken,
    notes: Vec<String>,
    // прогресс
    total_sec: f64,
    done_sec: f64,
    current: usize,
    count: usize,
}

impl Ctx<'_> {
    fn emit(&self, stage: &str, local: f64) {
        let pct = ((self.done_sec + local) / self.total_sec.max(0.001) * 100.0).clamp(0.0, 100.0);
        let _ = self.app.emit(
            "export-progress",
            Progress { percent: pct, stage: stage.into(), current: self.current, total: self.count },
        );
    }

    fn ext(&self) -> &'static str {
        if self.req.container == "mkv" { "mkv" } else { "mp4" }
    }

    fn note(&mut self, n: impl Into<String>) {
        let n = n.into();
        if !self.notes.contains(&n) {
            self.notes.push(n);
        }
    }

    async fn run(&self, cmd: tokio::process::Command, stage: &str, seg_dur: f64, offset: f64) -> AppResult<()> {
        run_progress(cmd, &self.token, |t| self.emit(stage, (offset + t).min(seg_dur))).await
    }
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "clip".into() } else { s }
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let p = dir.join(format!("{stem}.{ext}"));
    if !p.exists() {
        return p;
    }
    (2..10_000)
        .map(|i| dir.join(format!("{stem} ({i}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(p)
}

/// Кодеки звука, допустимые в MP4 при копировании.
fn mp4_audio_ok(codec: &str) -> bool {
    matches!(codec, "aac" | "mp3" | "ac3" | "eac3" | "opus" | "flac" | "alac")
}

// ---------- Команды ----------

#[tauri::command]
pub async fn trim_and_export(
    app: AppHandle,
    state: State<'_, AppState>,
    req: TrimRequest,
) -> AppResult<ExportResult> {
    if req.segments.is_empty() {
        return Err(AppError::msg("Нет фрагментов для экспорта"));
    }
    if let Some(s) = req.segments.iter().find(|s| s.end - s.start < 0.05) {
        return Err(AppError::msg(format!("Слишком короткий фрагмент: {:.2}–{:.2} с", s.start, s.end)));
    }
    let token = CancellationToken::new();
    {
        let mut g = state.export.lock().unwrap();
        if g.is_some() {
            return Err(AppError::msg("Экспорт уже выполняется"));
        }
        *g = Some(token.clone());
    }
    let started = Instant::now();
    let res = export(&app, &state, &req, token).await;
    *state.export.lock().unwrap() = None;
    res.map(|(files, notes)| ExportResult {
        files: files.into_iter().map(|p| p.to_string_lossy().into_owned()).collect(),
        notes,
        elapsed_sec: started.elapsed().as_secs_f64(),
    })
}

#[tauri::command]
pub fn cancel_export(state: State<'_, AppState>) {
    if let Some(t) = state.export.lock().unwrap().as_ref() {
        t.cancel();
    }
}

async fn export(
    app: &AppHandle,
    state: &AppState,
    req: &TrimRequest,
    token: CancellationToken,
) -> AppResult<(Vec<PathBuf>, Vec<String>)> {
    let meta = probe::probe(&req.input).await?;
    let out_dir = PathBuf::from(&req.output_dir);
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| AppError::msg(format!("Не удалось создать папку {}: {e}", out_dir.display())))?;

    let mut segs = req.segments.clone();
    for s in &mut segs {
        s.start = s.start.clamp(0.0, meta.duration);
        s.end = s.end.clamp(0.0, meta.duration);
    }
    let total: f64 = segs.iter().map(|s| s.end - s.start).sum();
    let merge = req.merge && segs.len() > 1;

    let mut ctx = Ctx {
        app,
        req,
        settings: state.settings(),
        meta,
        token,
        notes: Vec::new(),
        total_sec: total,
        done_sec: 0.0,
        current: 0,
        count: segs.len(),
    };

    // Для режима «целевой размер»: если файл всё-таки вышел больше — одна повторная попытка с поправкой.
    let size_mode = req.mode == TrimMode::Encode && req.encode.rate == RateKind::Size;
    let target_bytes = (req.encode.target_mb * 1024.0 * 1024.0) as u64;
    let mut factor = 1.0f64;

    for attempt in 0..2 {
        ctx.done_sec = 0.0;
        let files = run_all(&mut ctx, &segs, merge, &out_dir, factor).await?;
        if !size_mode || attempt == 1 {
            return Ok((files, ctx.notes));
        }
        let biggest = files
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok().map(|m| m.len()))
            .max()
            .unwrap_or(0);
        if biggest <= target_bytes {
            return Ok((files, ctx.notes));
        }
        factor *= target_bytes as f64 / biggest as f64 * 0.95;
        for f in &files {
            let _ = std::fs::remove_file(f);
        }
        ctx.note("Файл превысил лимит размера — выполнено повторное сжатие с меньшим битрейтом");
    }
    unreachable!()
}

async fn run_all(
    ctx: &mut Ctx<'_>,
    segs: &[Segment],
    merge: bool,
    out_dir: &Path,
    factor: f64,
) -> AppResult<Vec<PathBuf>> {
    let ext = ctx.ext();
    let base = sanitize(&ctx.req.base_name);
    let work = if merge {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let d = unique_dir(tmp_root(ctx.app)?.join(format!("export_{stamp}")));
        std::fs::create_dir_all(&d)?;
        Some(d)
    } else {
        None
    };

    let mut parts: Vec<PathBuf> = Vec::new();
    let result: AppResult<Vec<PathBuf>> = async {
        let merged_total: f64 = segs.iter().map(|s| s.end - s.start).sum();
        for (i, seg) in segs.iter().enumerate() {
            ctx.current = i + 1;
            let out = match &work {
                Some(d) => d.join(format!("part{i:03}.{ext}")),
                None => {
                    let suffix = seg
                        .name
                        .as_deref()
                        .filter(|n| !n.trim().is_empty())
                        .map(sanitize)
                        .unwrap_or_else(|| format!("{:02}", i + 1));
                    unique_path(out_dir, &format!("{base}_{suffix}"), ext)
                }
            };
            // Для склейки под целевой размер битрейт считается по суммарной длительности.
            let size_dur = if merge { merged_total } else { seg.end - seg.start };
            let r = export_segment(ctx, seg, &out, size_dur, factor).await;
            if r.is_err() {
                let _ = std::fs::remove_file(&out);
            }
            r?;
            parts.push(out);
            ctx.done_sec += seg.end - seg.start;
        }
        if let Some(_) = &work {
            ctx.emit("Склейка", 0.0);
            let fin = unique_path(out_dir, &format!("{base}_highlights"), ext);
            if let Err(e) = concat(ctx, &parts, &fin, false).await {
                let _ = std::fs::remove_file(&fin);
                return Err(e);
            }
            return Ok(vec![fin]);
        }
        Ok(parts.clone())
    }
    .await;

    if let Some(d) = &work {
        let _ = std::fs::remove_dir_all(d);
    } else if result.is_err() {
        for p in &parts {
            let _ = std::fs::remove_file(p);
        }
    }
    result
}

fn unique_dir(d: PathBuf) -> PathBuf {
    if !d.exists() {
        return d;
    }
    (1..1000)
        .map(|i| PathBuf::from(format!("{}_{i}", d.display())))
        .find(|p| !p.exists())
        .unwrap_or(d)
}

async fn export_segment(ctx: &mut Ctx<'_>, seg: &Segment, out: &Path, size_dur: f64, factor: f64) -> AppResult<()> {
    match ctx.req.mode {
        TrimMode::Copy => copy_segment(ctx, seg.start, seg.end, out, "Копирование").await,
        TrimMode::Smart => smart_segment(ctx, seg, out).await,
        TrimMode::Encode => encode_segment(ctx, seg, out, size_dur, factor).await,
    }
}

// ---------- 1. Мгновенный (stream copy) ----------

async fn copy_segment(ctx: &mut Ctx<'_>, start: f64, end: f64, out: &Path, stage: &str) -> AppResult<()> {
    let mut cmd = ffmpeg_cmd(Priority::BelowNormal, true)?;
    cmd.args(["-ss", &ts(start), "-to", &ts(end), "-i", &ctx.req.input]);
    cmd.args(["-map", "0:v:0?", "-map", "0:a?", "-sn", "-dn", "-c", "copy"]);
    if ctx.ext() == "mp4" && ctx.meta.audio_tracks.iter().any(|a| !mp4_audio_ok(&a.codec)) {
        cmd.args(["-c:a", "aac", "-b:a", "192k"]);
        ctx.note("Звук в формате, не поддерживаемом MP4, перекодирован в AAC");
    }
    cmd.args(["-avoid_negative_ts", "make_zero"]);
    cmd.args(container_args(ctx.ext(), false));
    cmd.arg(out);
    ctx.run(cmd, stage, end - start, 0.0).await
}

// ---------- 2. Умная склейка (Smart Cut) ----------

/// Первый ключевой кадр видео с меткой времени >= t (в секундах от начала файла).
async fn keyframe_at_or_after(input: &str, start_time: f64, t: f64) -> AppResult<Option<f64>> {
    let mut cmd = ffprobe_cmd()?;
    cmd.args([
        "-select_streams", "v:0",
        "-read_intervals", &format!("{:.3}%+20", (t + start_time - 0.05).max(0.0)),
        "-show_entries", "packet=pts_time,flags",
        "-of", "csv=p=0",
    ]);
    cmd.arg(input);
    let out = String::from_utf8_lossy(&output(cmd, None).await?).into_owned();
    let mut best: Option<f64> = None;
    for line in out.lines() {
        let mut it = line.split(',');
        let (Some(pts), Some(flags)) = (it.next(), it.next()) else { continue };
        if !flags.contains('K') {
            continue;
        }
        let Ok(pts) = pts.trim().parse::<f64>() else { continue };
        let rel = pts - start_time;
        if rel >= t - 0.001 && best.map_or(true, |b| rel < b) {
            best = Some(rel);
        }
    }
    Ok(best)
}

async fn smart_segment(ctx: &mut Ctx<'_>, seg: &Segment, out: &Path) -> AppResult<()> {
    let m = &ctx.meta;
    let video_ok = match m.video_codec.as_str() {
        "h264" => matches!(m.pix_fmt.as_str(), "yuv420p" | "yuvj420p"),
        "hevc" => matches!(m.pix_fmt.as_str(), "yuv420p" | "yuvj420p" | "yuv420p10le"),
        _ => false,
    };
    let audio_ok = m.audio_tracks.iter().all(|a| a.codec == "aac");
    if !m.has_video || !video_ok || !audio_ok {
        ctx.note("Умная склейка недоступна для этого формата — фрагмент перекодирован целиком в высоком качестве");
        return precise_full(ctx, seg, out).await;
    }

    let frame = 1.0 / m.fps.max(1.0);
    let key = keyframe_at_or_after(&ctx.req.input, m.start_time, seg.start).await?;
    let k = match key {
        // В пределах половины кадра — точка In уже на ключевом кадре.
        Some(k) if k - seg.start < frame * 0.5 => {
            return copy_segment(ctx, k, seg.end, out, "Копирование").await;
        }
        Some(k) if k < seg.end - frame => k,
        // Фрагмент целиком внутри одной GOP — перекодировать его полностью быстрее и надёжнее.
        _ => return precise_full(ctx, seg, out).await,
    };

    let dir = tmp_root(ctx.app)?;
    let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let head = dir.join(format!("{stem}.{}.head.ts", std::process::id()));
    let tailp = dir.join(format!("{stem}.{}.tail.ts", std::process::id()));
    let res = async {
        // Голова: [In, K) — перекодирование с параметрами, совпадающими с исходником.
        let mut cmd = ffmpeg_cmd(Priority::BelowNormal, true)?;
        cmd.args(["-ss", &ts(seg.start), "-to", &ts(k - 0.0005), "-i", &ctx.req.input]);
        cmd.args(["-map", "0:v:0", "-map", "0:a?", "-sn", "-dn"]);
        let threads = ctx.settings.cpu_threads;
        if ctx.meta.video_codec == "h264" {
            let profile = match ctx.meta.profile.as_deref().unwrap_or("") {
                p if p.contains("Baseline") => "baseline",
                p if p.contains("Main") => "main",
                _ => "high",
            };
            cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "16", "-profile:v", profile, "-pix_fmt", "yuv420p"]);
        } else {
            let ten = ctx.meta.pix_fmt.contains("10");
            cmd.args([
                "-c:v", "libx265", "-preset", "fast", "-crf", "17",
                "-x265-params", "log-level=error",
                "-pix_fmt", if ten { "yuv420p10le" } else { "yuv420p" },
            ]);
        }
        if threads > 0 {
            cmd.args(["-threads", &threads.to_string()]);
        }
        cmd.args(["-c:a", "aac", "-b:a", "192k", "-f", "mpegts"]).arg(&head);
        ctx.run(cmd, "Точная обрезка начала", seg.end - seg.start, 0.0).await?;

        // Хвост: [K, Out] — копирование без потерь.
        let mut cmd = ffmpeg_cmd(Priority::BelowNormal, true)?;
        cmd.args(["-ss", &ts(k + 0.0005), "-to", &ts(seg.end), "-i", &ctx.req.input]);
        cmd.args(["-map", "0:v:0", "-map", "0:a?", "-sn", "-dn", "-c", "copy", "-f", "mpegts"]).arg(&tailp);
        ctx.run(cmd, "Копирование", seg.end - seg.start, k - seg.start).await?;

        concat(ctx, &[head.clone(), tailp.clone()], out, true).await
    }
    .await;
    let _ = std::fs::remove_file(&head);
    let _ = std::fs::remove_file(&tailp);
    res
}

/// Точное перекодирование всего фрагмента в исходном кодеке-семействе (fallback для Smart Cut).
async fn precise_full(ctx: &mut Ctx<'_>, seg: &Segment, out: &Path) -> AppResult<()> {
    let enc = gpu::pick(&ctx.settings.encoders, "auto", "h264")
        .ok_or_else(|| AppError::msg("Нет кодировщика H.264"))?;
    let mut cmd = ffmpeg_cmd(ctx.settings.priority, true)?;
    cmd.args(["-ss", &ts(seg.start), "-to", &ts(seg.end), "-i", &ctx.req.input]);
    cmd.args(["-map", "0:v:0", "-map", "0:a?", "-sn", "-dn"]);
    cmd.args(["-vf", &format!("format={}", pix_fmt(&enc))]);
    cmd.args(video_args(&enc, &Rate::Quality(18), ctx.settings.cpu_threads));
    cmd.args(["-c:a", "aac", "-b:a", "192k"]);
    cmd.args(container_args(ctx.ext(), false));
    cmd.arg(out);
    ctx.run(cmd, "Перекодирование", seg.end - seg.start, 0.0).await
}

async fn concat(ctx: &mut Ctx<'_>, parts: &[PathBuf], out: &Path, from_ts: bool) -> AppResult<()> {
    let list = tmp_root(ctx.app)?.join(format!(
        "{}.{}.concat.txt",
        out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        std::process::id()
    ));
    let body: String = parts
        .iter()
        .map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\'', "'\\''")))
        .collect();
    std::fs::write(&list, body)?;
    let mut cmd = ffmpeg_cmd(Priority::BelowNormal, true)?;
    cmd.args(["-f", "concat", "-safe", "0", "-i"]).arg(&list);
    cmd.args(["-map", "0", "-c", "copy"]);
    if from_ts && ctx.ext() == "mp4" && !ctx.meta.audio_tracks.is_empty() {
        cmd.args(["-bsf:a", "aac_adtstoasc"]);
    }
    let hevc = ctx.meta.video_codec == "hevc" && ctx.req.mode != TrimMode::Encode
        || ctx.req.mode == TrimMode::Encode && ctx.req.encode.codec == "hevc";
    cmd.args(container_args(ctx.ext(), hevc));
    cmd.arg(out);
    let r = run_progress(cmd, &ctx.token, |_| {}).await;
    let _ = std::fs::remove_file(&list);
    r
}

// ---------- 3. Перекодирование (GPU) ----------

fn audio_graph(meta: &VideoMeta, s: &EncodeSettings) -> Option<String> {
    if s.mute || meta.audio_tracks.is_empty() {
        return None;
    }
    let mut sel: Vec<(usize, f32)> = s
        .audio_tracks
        .iter()
        .filter(|t| t.enabled && t.index < meta.audio_tracks.len())
        .map(|t| (t.index, t.volume.clamp(0.0, 4.0)))
        .collect();
    if s.audio_tracks.is_empty() {
        sel = meta.audio_tracks.iter().map(|t| (t.index, 1.0)).collect();
    }
    match sel.len() {
        0 => None,
        1 => Some(format!("[0:a:{}]volume={:.3}[a]", sel[0].0, sel[0].1)),
        n => {
            let mut g: Vec<String> = sel
                .iter()
                .enumerate()
                .map(|(k, (i, v))| format!("[0:a:{i}]volume={v:.3}[a{k}]"))
                .collect();
            let ins: String = (0..n).map(|k| format!("[a{k}]")).collect();
            g.push(format!("{ins}amix=inputs={n}:normalize=0:dropout_transition=0[a]"));
            Some(g.join(";"))
        }
    }
}

async fn encode_segment(ctx: &mut Ctx<'_>, seg: &Segment, out: &Path, size_dur: f64, factor: f64) -> AppResult<()> {
    let s = ctx.req.encode.clone();
    let m = ctx.meta.clone();
    if !m.has_video {
        return Err(AppError::msg("В файле нет видеодорожки"));
    }
    let dur = seg.end - seg.start;
    let audio = audio_graph(&m, &s);
    let audio_kbps = if audio.is_some() { s.audio_bitrate_kbps.clamp(32, 512) } else { 0 };

    // Режим битрейта
    let mut height = s.height.filter(|h| *h > 0);
    let rate = match s.rate {
        RateKind::Quality => Rate::Quality(s.quality.min(51)),
        RateKind::Bitrate => {
            let k = s.bitrate_kbps.clamp(100, 200_000);
            Rate::Bitrate { kbps: k, max_kbps: k * 3 / 2 }
        }
        RateKind::Size => {
            // Bitrate = (MB * 8192 / сек) − звук, запас 3% на контейнер.
            let total = s.target_mb * 8192.0 / size_dur.max(0.1) * 0.97;
            let v = ((total - audio_kbps as f64) * factor).floor();
            if v < 120.0 {
                return Err(AppError::msg(format!(
                    "Слишком маленький размер ({} МБ) для {:.0} с видео — увеличьте лимит или укоротите клип",
                    s.target_mb, size_dur
                )));
            }
            let v = v as u32;
            // При низком битрейте уменьшаем разрешение, иначе картинка рассыплется на блоки.
            if height.is_none() {
                let src_h = s.crop.as_ref().map(|c| c.h).unwrap_or(m.display_height);
                let auto = if v < 700 { 480 } else if v < 1500 { 720 } else { 0 };
                if auto > 0 && src_h > auto {
                    height = Some(auto);
                    ctx.note(format!("Для целевого размера разрешение автоматически снижено до {auto}p"));
                }
            }
            Rate::Bitrate { kbps: v, max_kbps: v + v / 10 }
        }
    };

    let primary = gpu::pick(&ctx.settings.encoders, &s.encoder, &s.codec).ok_or_else(|| {
        AppError::msg(format!("Кодировщик {} для {} недоступен на этом ПК", s.encoder, s.codec.to_uppercase()))
    })?;

    // Попытки: GPU-декодирование + GPU-кодирование → без hwaccel → CPU-кодировщик (если выбран «Авто»).
    let mut attempts: Vec<(EncoderInfo, bool)> = vec![(primary.clone(), true), (primary.clone(), false)];
    if primary.hardware && s.encoder == "auto" {
        if let Some(cpu) = gpu::pick(&ctx.settings.encoders, "cpu", &s.codec) {
            attempts.push((cpu, false));
        }
    }

    let out_fps = s.fps.filter(|f| *f > 0.0).unwrap_or(m.fps).max(1.0);
    let mut last_err = None;
    for (enc, hwaccel) in attempts {
        let mut vf: Vec<String> = Vec::new();
        if let Some(c) = &s.crop {
            let w = (c.w.min(m.display_width) / 2) * 2;
            let h = (c.h.min(m.display_height) / 2) * 2;
            let x = (c.x.min(m.display_width - w) / 2) * 2;
            let y = (c.y.min(m.display_height - h) / 2) * 2;
            if w < m.display_width || h < m.display_height {
                vf.push(format!("crop={w}:{h}:{x}:{y}"));
            }
        }
        if let Some(h) = height {
            vf.push(format!("scale=-2:{}:flags=bicubic", h - h % 2));
        }
        if let Some(f) = s.fps.filter(|f| *f > 0.0) {
            if (f - m.fps).abs() > 0.01 {
                vf.push(format!("fps={f}"));
            }
        }
        vf.push(format!("format={}", pix_fmt(&enc)));
        let mut graph = format!("[0:v:0]{}[v]", vf.join(","));
        if let Some(a) = &audio {
            graph.push(';');
            graph.push_str(a);
        }

        let mut cmd = ffmpeg_cmd(ctx.settings.priority, true)?;
        if hwaccel {
            cmd.args(["-hwaccel", "auto"]);
        }
        cmd.args(["-ss", &ts(seg.start), "-to", &ts(seg.end), "-i", &ctx.req.input]);
        cmd.args(["-filter_complex", &graph, "-map", "[v]"]);
        if audio.is_some() {
            cmd.args(["-map", "[a]", "-c:a", "aac", "-b:a", &format!("{audio_kbps}k")]);
        } else {
            cmd.arg("-an");
        }
        cmd.args(video_args(&enc, &rate, ctx.settings.cpu_threads));
        cmd.args(["-g", &((out_fps * 2.0).round() as u32).to_string()]);
        cmd.args(["-avoid_negative_ts", "make_zero"]);
        cmd.args(container_args(ctx.ext(), enc.codec == "hevc"));
        cmd.arg(out);

        let stage = if enc.hardware {
            format!("Кодирование на GPU ({})", enc.name)
        } else {
            format!("Кодирование на CPU ({})", enc.name)
        };
        match ctx.run(cmd, &stage, dur, 0.0).await {
            Ok(()) => {
                if !enc.hardware && primary.hardware {
                    ctx.note(format!("GPU-кодировщик {} дал сбой — использован {}", primary.name, enc.name));
                }
                return Ok(());
            }
            Err(AppError::Cancelled) => return Err(AppError::Cancelled),
            Err(e) => {
                let _ = std::fs::remove_file(out);
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::msg("Не удалось закодировать фрагмент")))
}
