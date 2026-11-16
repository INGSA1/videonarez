import type { AspectKey, ExportOptions } from "./types";
import { clamp } from "./format";

export const MAX_CROP_ZOOM = 400;

export const FRAME_DEFAULTS = {
  aspect: "none",
  cropX: 0.5,
  cropY: 0.5,
  cropZoom: 100,
} as const;

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
    ...FRAME_DEFAULTS,
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
    apply: { mode: "encode", codec: "h264", rate: "size", targetMb: 10, height: null, fps: null, audioBitrateKbps: 96, container: "mp4" },
  },
  discord50: {
    label: "Discord Nitro, до 50 МБ",
    hint: "H.264 под лимит Nitro Basic",
    apply: { mode: "encode", codec: "h264", rate: "size", targetMb: 50, height: null, fps: null, audioBitrateKbps: 128, container: "mp4" },
  },
  telegram: {
    label: "Telegram",
    hint: "H.264 1080p, хорошее качество, быстро грузится",
    apply: { mode: "encode", codec: "h264", rate: "quality", quality: 24, height: 1080, fps: null, audioBitrateKbps: 128, container: "mp4" },
  },
  shorts: {
    label: "Shorts, TikTok, Reels",
    hint: "Вертикаль 9:16, 1080×1920, высокое качество",
    apply: { mode: "encode", codec: "h264", rate: "quality", quality: 20, height: 1920, fps: null, audioBitrateKbps: 192, container: "mp4", aspect: "9:16" },
  },
  youtube: {
    label: "YouTube и архив",
    hint: "Исходное разрешение, высокое качество",
    apply: { mode: "encode", codec: "h264", rate: "quality", quality: 19, height: null, fps: null, audioBitrateKbps: 192, container: "mp4" },
  },
  hevcSmall: {
    label: "Компактный HEVC",
    hint: "H.265 — на ~35% меньше при том же качестве",
    apply: { mode: "encode", codec: "hevc", rate: "quality", quality: 25, height: null, fps: null, audioBitrateKbps: 160, container: "mp4" },
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

export const FRAME_ASPECTS = Object.keys(ASPECTS) as AspectKey[];

/** Область исходника и размер готового кадра. Размеры и смещения кратны двум. */
export function cropRect(
  width: number,
  height: number,
  { aspect, cropX, cropY, cropZoom, height: outputHeight }: Pick<ExportOptions, "aspect" | "cropX" | "cropY" | "cropZoom" | "height">,
) {
  if (width < 2 || height < 2) return null;
  const ratio = ASPECTS[aspect] ?? width / height;
  const zoom = clamp(cropZoom, 100, MAX_CROP_ZOOM) / 100;
  if (aspect === "none" && zoom === 1) return null;
  const fittedW = Math.min(width, height * ratio);
  const fittedH = Math.min(height, width / ratio);
  const even = (n: number) => Math.floor(Math.round(n) / 2) * 2;
  const w = Math.max(2, even(fittedW / zoom));
  const h = Math.max(2, even(fittedH / zoom));
  if (w === width && h === height) return null;
  const x = even((width - w) * clamp(cropX, 0, 1));
  const y = even((height - h) * clamp(cropY, 0, 1));
  const frameH = Math.max(2, even(outputHeight || fittedH));
  const frameW = outputHeight ? Math.round(frameH * ratio / 2) * 2 : even(fittedW);
  return { x, y, w, h, frameSize: { w: Math.max(2, frameW), h: frameH } };
}

export const HEIGHTS = [2160, 1440, 1080, 720, 480];
export const VERTICAL_HEIGHTS = [1920, 1280, 1080];
export const FPS_OPTIONS = [30, 60, 120];
