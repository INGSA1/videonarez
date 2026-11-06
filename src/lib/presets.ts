import type { AspectKey, ExportOptions } from "./types";

export function defaultExport(): ExportOptions {
  return {
    preset: "custom",
    mode: "copy",
    container: "mp4",
    merge: false,
    encoder: "auto",
    codec: "h264",
    height: null,
    fps: null,
    aspect: "none",
    cropX: 0.5,
    cropY: 0.5,
    rate: "quality",
    quality: 23,
    bitrateKbps: 8000,
    targetMb: 10,
    audioBitrateKbps: 160,
    mute: false,
    mixdown: true,
    tracks: {},
  };
}

export interface Preset {
  label: string;
  hint: string;
  apply: Partial<ExportOptions>;
}

export const PRESETS: Record<string, Preset> = {
  discord10: {
    label: "Discord, до 10 МБ",
    hint: "H.264, сжатие точно под лимит бесплатного Discord",
    apply: { mode: "encode", codec: "h264", rate: "size", targetMb: 10, height: null, fps: null, audioBitrateKbps: 96, container: "mp4", aspect: "none" },
  },
  discord50: {
    label: "Discord Nitro, до 50 МБ",
    hint: "H.264 под лимит Nitro Basic",
    apply: { mode: "encode", codec: "h264", rate: "size", targetMb: 50, height: null, fps: null, audioBitrateKbps: 128, container: "mp4", aspect: "none" },
  },
  telegram: {
    label: "Telegram",
    hint: "H.264 1080p, хорошее качество, быстро грузится",
    apply: { mode: "encode", codec: "h264", rate: "quality", quality: 24, height: 1080, fps: null, audioBitrateKbps: 128, container: "mp4", aspect: "none" },
  },
  shorts: {
    label: "Shorts, TikTok, Reels",
    hint: "Вертикаль 9:16, 1080×1920, высокое качество",
    apply: { mode: "encode", codec: "h264", rate: "quality", quality: 20, height: 1920, fps: null, audioBitrateKbps: 192, container: "mp4", aspect: "9:16" },
  },
  youtube: {
    label: "YouTube и архив",
    hint: "Исходное разрешение, высокое качество",
    apply: { mode: "encode", codec: "h264", rate: "quality", quality: 19, height: null, fps: null, audioBitrateKbps: 192, container: "mp4", aspect: "none" },
  },
  hevcSmall: {
    label: "Компактный HEVC",
    hint: "H.265 — на ~35% меньше при том же качестве",
    apply: { mode: "encode", codec: "hevc", rate: "quality", quality: 25, height: null, fps: null, audioBitrateKbps: 160, container: "mp4", aspect: "none" },
  },
};

export const ASPECTS: Record<AspectKey, number | null> = {
  none: null,
  "9:16": 9 / 16,
  "1:1": 1,
  "4:5": 4 / 5,
  "16:9": 16 / 9,
  "4:3": 4 / 3,
};

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Прямоугольник кадрирования в пикселях исходника (чётные значения). cx/cy — положение 0..1. */
export function cropRect(W: number, H: number, aspect: AspectKey, cx: number, cy: number): Rect | null {
  const a = ASPECTS[aspect];
  if (!a || !W || !H) return null;
  let w: number, h: number;
  if (W / H > a) {
    h = H;
    w = Math.round(H * a);
  } else {
    w = W;
    h = Math.round(W / a);
  }
  w -= w % 2;
  h -= h % 2;
  let x = Math.round((W - w) * cx);
  let y = Math.round((H - h) * cy);
  x -= x % 2;
  y -= y % 2;
  return { x, y, w, h };
}

export const HEIGHTS = [2160, 1440, 1080, 720, 480];
export const VERTICAL_HEIGHTS = [1920, 1280, 1080];
export const FPS_OPTIONS = [30, 60, 120];
