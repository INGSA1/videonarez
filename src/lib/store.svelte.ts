import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { clamp } from "./format";
import { cropRect, defaultExport, PRESETS } from "./presets";
import type {
  CaptureSettings,
  CaptureSources,
  CaptureStatus,
  ClipSaved,
  ExportOptions,
  ExportProgress,
  ExportResult,
  Job,
  Segment,
  Settings,
  Thumb,
  VideoMeta,
  Waveform,
} from "./types";

export const VIDEO_EXT = ["mp4", "mkv", "mov", "webm", "avi", "m4v", "ts", "m2ts", "flv", "wmv", "mpg", "mpeg", "3gp"];

export const app = $state({
  meta: null as VideoMeta | null,
  src: "",
  usingProxy: false,
  proxyPercent: -1,
  loading: false,
  error: "",
  toast: "",

  thumbs: [] as Thumb[],
  waveform: null as Waveform | null,
  /** Волна каждой звуковой дорожки по её индексу. */
  waveforms: {} as Record<number, Waveform>,
  /** Отдельные файлы дорожек для просмотра (только если дорожек больше одной). */
  trackUrls: [] as string[],
  waveLoading: false,
  waveTrack: 0,

  currentTime: 0,
  playing: false,
  playRange: false,

  inPoint: 0,
  outPoint: 0,
  segments: [] as Segment[],
  selectedId: null as number | null,
  view: { start: 0, end: 1 },

  settings: null as Settings | null,
  opts: defaultExport() as ExportOptions,
  baseName: "",

  exporting: false,
  progress: { percent: 0, stage: "", current: 0, total: 0 } as ExportProgress,
  result: null as ExportResult | null,
  queue: [] as Job[],
  currentJob: null as number | null,
  autostart: false,

  tab: "clips" as "clips" | "export" | "queue",
  showSettings: false,
  dragOver: false,

  version: "",
  update: {
    info: null as { version: string; current: string; notes?: string | null; date?: string | null } | null,
    checking: false,
    installing: false,
    downloaded: 0,
    total: 0,
    message: "",
    dismissed: false,
  },

  mode: "editor" as "editor" | "capture",
  cap: {
    status: null as CaptureStatus | null,
    sources: null as CaptureSources | null,
    draft: null as CaptureSettings | null,
    dirty: false,
    applying: false,
    busy: false,
    recent: [] as ClipSaved[],
    hotkeyErrors: [] as string[],
  },
});

/** Ссылка на <video> (не реактивная). */
export const player = { el: null as HTMLVideoElement | null };

let nextId = 1;
let toastTimer = 0;

export function toast(msg: string) {
  app.toast = msg;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (app.toast = ""), 3500);
}

// ---------- Инициализация ----------

export async function init() {
  await listen<Thumb & { path: string }>("thumbnail", (e) => {
    if (e.payload.path !== app.meta?.path) return;
    app.thumbs.push({ index: e.payload.index, time: e.payload.time, data: e.payload.data });
  });
  await listen<ExportProgress>("export-progress", (e) => {
    app.progress = e.payload;
  });
  await listen<{ path: string; percent: number }>("proxy-progress", (e) => {
    if (e.payload.path === app.meta?.path) app.proxyPercent = e.payload.percent;
  });

  await listen<CaptureStatus>("capture-status", (e) => {
    app.cap.status = e.payload;
  });
  await listen<ClipSaved>("clip-saved", (e) => {
    app.cap.recent.unshift(e.payload);
    if (app.cap.recent.length > 50) app.cap.recent.pop();
    toast(`${e.payload.kind === "replay" ? "Повтор" : "Запись"} сохранён(а): ${e.payload.path.split(/[\\/]/).pop()}`);
  });
  await listen<string>("capture-error", (e) => {
    app.error = e.payload;
  });
  await listen<Job[]>("queue-update", (e) => onQueue(e.payload));
  await listen<NonNullable<typeof app.update.info>>("update-available", (e) => {
    app.update.info = e.payload;
  });
  await listen<{ downloaded: number; total: number | null }>("update-progress", (e) => {
    app.update.downloaded = e.payload.downloaded;
    app.update.total = e.payload.total ?? 0;
  });
  invoke<string>("app_version").then((v) => (app.version = v)).catch(() => {});
  // Тихая проверка при открытии окна: ошибки сети здесь не показываем.
  void checkUpdates(false);
  invoke<Job[]>("queue_list").then(onQueue).catch(() => {});
  // История клипов хранится в ядре — переживает закрытие окна в трей.
  invoke<ClipSaved[]>("capture_recent").then((r) => (app.cap.recent = r)).catch(() => {});
  invoke<boolean>("get_autostart").then((v) => (app.autostart = v)).catch(() => {});

  try {
    app.settings = await invoke<Settings>("get_settings");
    const ui = app.settings.ui;
    if (ui && typeof ui === "object" && ui.opts) {
      app.opts = { ...defaultExport(), ...ui.opts, tracks: {} };
    }
    if (ui && typeof ui === "object" && ui.mode === "capture") app.mode = "capture";
    app.cap.draft = structuredClone($state.snapshot(app.settings.capture));
  } catch (e) {
    app.error = String(e);
  }
  invoke<CaptureStatus>("capture_status").then((s) => (app.cap.status = s)).catch(() => {});
  void refreshSources();
  // Определение GPU-кодировщиков (результат кэшируется; первый запуск — пара секунд в фоне).
  invoke<Settings["encoders"]>("detect_encoders", { force: false })
    .then((list) => app.settings && (app.settings.encoders = list))
    .catch(() => {});

  const file = await invoke<string | null>("startup_file").catch(() => null);
  if (file) await openPath(file);
}

let saveTimer = 0;
export function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(saveSettings, 600);
}

export async function saveSettings() {
  if (!app.settings) return;
  const { tracks: _t, ...opts } = $state.snapshot(app.opts);
  const s = { ...$state.snapshot(app.settings), ui: { opts, mode: app.mode } };
  try {
    await invoke("save_settings", { settings: s });
  } catch (e) {
    console.error(e);
  }
}

// ---------- Открытие файла ----------

export async function openFileDialog() {
  const path = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "Видео", extensions: VIDEO_EXT }],
  });
  if (typeof path === "string") await openPath(path);
}

export async function openPath(path: string) {
  // Экспорт может идти в очереди — новый файл открывается сразу; результат прошлого
  // задания останется во вкладке «Очередь».
  app.currentJob = null;
  app.exporting = false;
  const el = player.el;
  el?.pause();
  app.loading = true;
  app.error = "";
  app.result = null;
  try {
    const meta = await invoke<VideoMeta>("probe_video_metadata", { path });
    app.meta = meta;
    app.thumbs = [];
    app.waveform = null;
    app.waveforms = {};
    app.trackUrls = [];
    app.waveTrack = 0;
    app.segments = [];
    app.selectedId = null;
    app.inPoint = 0;
    app.outPoint = meta.duration;
    app.currentTime = 0;
    app.view = { start: 0, end: meta.duration };
    app.baseName = meta.fileName.replace(/\.[^.]+$/, "");
    app.usingProxy = false;
    app.proxyPercent = -1;
    app.opts.tracks = Object.fromEntries(meta.audioTracks.map((t) => [t.index, { enabled: true, volume: 1 }]));

    if (meta.needsProxy) {
      app.src = "";
      void usePreviewProxy(false);
    } else {
      app.src = convertFileSrc(path, "clip");
    }
    void analyze(meta);
  } catch (e) {
    app.error = String(e);
  } finally {
    app.loading = false;
  }
}

async function analyze(meta: VideoMeta) {
  const path = meta.path;
  if (meta.hasVideo) {
    const count = clamp(Math.ceil(window.innerWidth / 110), 12, 40);
    invoke("extract_thumbnails", { path, count, duration: meta.duration, height: 90 }).catch(() => {});
  }
  if (!meta.audioTracks.length) return;
  // Несколько дорожек: каждую выносим в отдельный файл, чтобы слышать их все при просмотре.
  if (meta.audioTracks.length > 1) {
    invoke<string[]>("extract_audio_tracks", { path })
      .then((files) => {
        if (app.meta?.path === path) app.trackUrls = files.map((f) => convertFileSrc(f, "clip"));
      })
      .catch((e) => {
        if (!String(e).includes("Отменено")) console.warn("tracks", e);
      });
  }
  // Волна каждой дорожки; метки громкости берутся с выбранной (по умолчанию первой).
  app.waveLoading = true;
  try {
    for (const t of meta.audioTracks) {
      const wf = await invoke<Waveform>("extract_waveform", { path, track: t.index, duration: meta.duration });
      if (app.meta?.path !== path) return;
      app.waveforms[t.index] = wf;
      if (t.index === app.waveTrack) app.waveform = wf;
    }
  } catch (e) {
    if (!String(e).includes("Отменено")) console.warn("waveform", e);
  } finally {
    if (app.meta?.path === path) app.waveLoading = false;
  }
}

/** Чья волна даёт метки громких моментов. */
export function loadWaveform(track: number) {
  app.waveTrack = track;
  app.waveform = app.waveforms[track] ?? null;
}

/**
 * Плеер не смог воспроизвести источник. Лестница деградации:
 * оригинал → ремукс-прокси (секунды) → перекодированный 720p-прокси.
 */
export async function onVideoError() {
  if (!app.meta || app.proxyPercent >= 0) return;
  if (!app.usingProxy) await usePreviewProxy(false);
  else if (!app.src.endsWith("?v=t")) await usePreviewProxy(true);
  else app.error = "WebView2 не может воспроизвести этот файл даже после перекодирования";
}

async function usePreviewProxy(transcode: boolean) {
  const meta = app.meta;
  if (!meta) return;
  const path = meta.path;
  app.proxyPercent = 0;
  try {
    const proxy = await invoke<string>("prepare_preview", { path, transcode });
    if (app.meta?.path !== path) return;
    app.usingProxy = true;
    // Запрос ?v=… сервер игнорирует, он лишь помечает перекодированный прокси.
    app.src = convertFileSrc(proxy, "clip") + (transcode ? "?v=t" : "?v=r");
  } catch (e) {
    if (app.meta?.path === path && !String(e).includes("Отменено")) {
      app.error = `Не удалось подготовить предпросмотр: ${e}`;
    }
  } finally {
    if (app.meta?.path === path) app.proxyPercent = -1;
  }
}

// ---------- Плеер ----------

let seekPending: number | null = null;
let seekRaf = 0;

/** Перемотка с объединением запросов по кадрам анимации (скраббинг не забивает декодер). */
export function seek(t: number) {
  const m = app.meta;
  if (!m) return;
  t = clamp(t, 0, m.duration);
  app.currentTime = t;
  seekPending = t;
  if (!seekRaf) {
    seekRaf = requestAnimationFrame(() => {
      seekRaf = 0;
      const el = player.el;
      if (el && seekPending !== null && Number.isFinite(el.duration)) el.currentTime = seekPending;
      seekPending = null;
    });
  }
}

export function togglePlay() {
  const el = player.el;
  if (!el || !app.src) return;
  app.playRange = false;
  if (el.paused) void el.play().catch(() => {});
  else el.pause();
}

export function playRange() {
  const el = player.el;
  if (!el) return;
  const r = curRange();
  el.currentTime = r.start;
  app.currentTime = r.start;
  app.playRange = true;
  void el.play().catch(() => {});
}

export function step(frames: number) {
  const m = app.meta;
  if (!m) return;
  player.el?.pause();
  seek(app.currentTime + frames / (m.fps || 30));
}

export function skip(sec: number) {
  seek(app.currentTime + sec);
}

// ---------- Диапазон и клипы ----------

const MIN_LEN = 0.1;

export function selected(): Segment | undefined {
  return app.segments.find((s) => s.id === app.selectedId);
}

export function curRange() {
  const s = selected();
  return s ? { start: s.start, end: s.end } : { start: app.inPoint, end: app.outPoint };
}

export function setIn(t: number) {
  const m = app.meta;
  if (!m) return;
  t = clamp(t, 0, m.duration);
  const s = selected();
  if (s) {
    s.start = Math.min(t, s.end - MIN_LEN);
    return;
  }
  app.inPoint = t;
  if (app.outPoint - t < MIN_LEN) app.outPoint = m.duration;
}

export function setOut(t: number) {
  const m = app.meta;
  if (!m) return;
  t = clamp(t, 0, m.duration);
  const s = selected();
  if (s) {
    s.end = Math.max(t, s.start + MIN_LEN);
    return;
  }
  app.outPoint = t;
  if (t - app.inPoint < MIN_LEN) app.inPoint = 0;
}

export function addSegment(start?: number, end?: number) {
  const m = app.meta;
  if (!m) return;
  const r = curRange();
  start = start ?? r.start;
  end = end ?? r.end;
  if (end - start < MIN_LEN) return;
  const seg: Segment = { id: nextId++, start, end, name: "" };
  app.segments.push(seg);
  app.segments.sort((a, b) => a.start - b.start);
  app.selectedId = null;
  toast(`Клип добавлен, в списке ${app.segments.length}`);
}

export function removeSegment(id: number) {
  app.segments = app.segments.filter((s) => s.id !== id);
  if (app.selectedId === id) app.selectedId = null;
}

export function selectSegment(id: number | null) {
  app.selectedId = app.selectedId === id ? null : id;
}

/** Клип вокруг громкого пика: 8 с до и 4 с после. */
export function addSegmentAround(t: number, before = 8, after = 4) {
  const m = app.meta;
  if (!m) return;
  addSegment(Math.max(0, t - before), Math.min(m.duration, t + after));
}

/** Создать клипы из всех найденных пиков звука (пересекающиеся объединяются). */
export function segmentsFromPeaks(minScore = 0.35) {
  const m = app.meta;
  const wf = app.waveform;
  if (!m || !wf?.markers.length) return;
  const ranges = wf.markers
    .filter((mk) => mk.score >= minScore)
    .map((mk) => ({ start: Math.max(0, mk.time - 8), end: Math.min(m.duration, mk.time + 4) }))
    .sort((a, b) => a.start - b.start);
  const merged: { start: number; end: number }[] = [];
  for (const r of ranges) {
    const last = merged[merged.length - 1];
    if (last && r.start <= last.end) last.end = Math.max(last.end, r.end);
    else merged.push({ ...r });
  }
  for (const r of merged) app.segments.push({ id: nextId++, start: r.start, end: r.end, name: "" });
  app.segments.sort((a, b) => a.start - b.start);
  toast(`Добавлено клипов из пиков: ${merged.length}`);
}

// ---------- Экспорт ----------

export function applyPreset(key: string) {
  const p = PRESETS[key];
  app.opts.preset = key;
  if (p) Object.assign(app.opts, p.apply);
}

export function exportSegments() {
  return app.segments.length
    ? app.segments.map((s) => ({ start: s.start, end: s.end, name: s.name || null }))
    : [{ start: app.inPoint, end: app.outPoint, name: null }];
}

export async function startExport() {
  const m = app.meta;
  const st = app.settings;
  if (!m || !st) return; // идущий экспорт не мешает: новое задание встаёт в очередь
  const o = app.opts;
  const crop = o.mode === "encode" ? cropRect(m.displayWidth, m.displayHeight, o.aspect, o.cropX, o.cropY) : null;
  const segments = exportSegments();
  const req = {
    input: m.path,
    segments,
    merge: o.merge && segments.length > 1,
    mode: o.mode,
    container: o.container,
    outputDir: st.outputDir,
    baseName: app.baseName || "clip",
    encode: {
      encoder: o.encoder,
      codec: o.codec,
      height: o.height,
      fps: o.fps,
      crop,
      rate: o.rate,
      quality: o.quality,
      bitrateKbps: o.bitrateKbps,
      targetMb: o.targetMb,
      audioTracks: m.audioTracks.map((t) => ({
        index: t.index,
        volume: o.tracks[t.index]?.volume ?? 1,
        enabled: o.tracks[t.index]?.enabled ?? true,
      })),
      audioBitrateKbps: o.audioBitrateKbps,
      mute: o.mute,
      mixdown: o.mixdown,
    },
  };
  player.el?.pause();
  app.result = null;
  app.error = "";
  const busy = app.queue.some((j) => j.status === "running" || j.status === "queued");
  const label = segments.length > 1 ? `${m.fileName}, клипов: ${segments.length}` : m.fileName;
  try {
    const id = await invoke<number>("queue_add", { req, label });
    app.currentJob = id;
    app.exporting = true;
    if (!busy) app.progress = { percent: 0, stage: "Подготовка", current: 0, total: segments.length };
    else toast("Добавлено в очередь экспорта");
  } catch (e) {
    app.error = String(e);
  }
}

/** Обновление очереди: следим за заданием текущего файла, чтобы показать его результат. */
function onQueue(list: Job[]) {
  const prevRunning = app.queue.find((j) => j.status === "running")?.id;
  app.queue = list;
  const nowRunning = list.find((j) => j.status === "running")?.id;
  if (nowRunning !== undefined && nowRunning !== prevRunning) {
    app.progress = { percent: 0, stage: "Подготовка", current: 0, total: 0 };
  }
  const cur = list.find((j) => j.id === app.currentJob);
  if (!cur) {
    app.exporting = false;
    return;
  }
  app.exporting = cur.status === "queued" || cur.status === "running";
  if (cur.status === "done") {
    app.result = { files: cur.files, notes: cur.notes, elapsedSec: cur.elapsedSec };
    app.currentJob = null;
  } else if (cur.status === "failed") {
    app.error = cur.error ?? "Ошибка экспорта";
    app.currentJob = null;
  } else if (cur.status === "cancelled") {
    toast("Экспорт отменён");
    app.currentJob = null;
  }
}

export function cancelExport(id?: number) {
  const target = id ?? app.currentJob;
  if (target != null) void invoke("queue_cancel", { id: target });
}

export function clearQueue() {
  void invoke("queue_clear");
}

/** Положить файл клипа в буфер обмена, чтобы вставить его в Discord через Ctrl+V. */
export async function copyForDiscord(path: string) {
  try {
    const bytes = await invoke<number>("copy_file_to_clipboard", { path });
    const mb = bytes / 1048576;
    toast(
      mb > 10
        ? `Скопировано, ${mb.toFixed(0)} МБ. Без Nitro Discord принимает до 10 МБ: сожмите готовой настройкой «Discord, до 10 МБ».`
        : "Скопировано. Вставьте в Discord через Ctrl+V.",
    );
  } catch (e) {
    app.error = String(e);
  }
}

// ---------- Обновления ----------

export async function checkUpdates(manual: boolean) {
  app.update.checking = true;
  app.update.message = "";
  try {
    const info = await invoke<typeof app.update.info>("check_update");
    app.update.info = info;
    if (!info && manual) app.update.message = "Установлена последняя версия";
  } catch (e) {
    if (manual) app.update.message = String(e);
  } finally {
    app.update.checking = false;
  }
}

export async function installUpdate() {
  app.update.installing = true;
  app.update.downloaded = 0;
  try {
    // После установки приложение перезапустится само; возврат сюда бывает только в режиме проверки.
    const bytes = await invoke<number>("install_update", { dryRun: false });
    app.update.installing = false;
    toast(`Обновление скачано и подпись проверена, ${(bytes / 1048576).toFixed(1)} МБ. Установка пропущена (режим проверки).`);
  } catch (e) {
    app.update.installing = false;
    app.error = String(e);
  }
}

export async function setAutostart(enabled: boolean) {
  try {
    app.autostart = await invoke<boolean>("set_autostart", { enabled });
  } catch (e) {
    app.error = String(e);
  }
}

export function reveal(path: string) {
  void invoke("reveal_in_explorer", { path });
}

// ---------- Захват и буфер повтора ----------

export async function refreshSources() {
  try {
    app.cap.sources = await invoke<CaptureSources>("capture_sources");
  } catch (e) {
    console.warn(e);
  }
}

async function capCall<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
  app.cap.busy = true;
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    app.error = String(e);
  } finally {
    app.cap.busy = false;
  }
}

export async function captureToggle() {
  const running = app.cap.status?.running || app.cap.status?.waitingForGame;
  const st = await capCall<CaptureStatus>(running ? "capture_stop" : "capture_start");
  if (st) app.cap.status = st;
}

export async function captureSave() {
  await capCall("capture_save", { secs: null });
}

export async function captureRecord() {
  await capCall("capture_toggle_record");
}

/** Применить настройки захвата: сохранить, перерегистрировать хоткеи, перезапустить захват. */
export async function applyCapture() {
  const d = app.cap.draft;
  if (!d || !app.settings) return;
  app.cap.applying = true;
  try {
    const errors = await invoke<string[]>("capture_apply_settings", { capture: $state.snapshot(d) });
    app.settings.capture = structuredClone($state.snapshot(d));
    app.cap.hotkeyErrors = errors;
    app.cap.dirty = false;
    toast(errors.length ? "Настройки применены, но часть горячих клавиш занята" : "Настройки захвата применены");
  } catch (e) {
    app.error = String(e);
  } finally {
    app.cap.applying = false;
  }
}

export async function openInEditor(path: string) {
  app.mode = "editor";
  await openPath(path);
}
