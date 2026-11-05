const pad = (n: number, w = 2) => String(n).padStart(w, "0");

/** 1:02:03.45 / 02:03.45 */
export function fmtTime(t: number, frac = 2): string {
  if (!Number.isFinite(t)) t = 0;
  const neg = t < 0;
  t = Math.abs(t);
  const scale = 10 ** frac;
  let total = Math.round(t * scale);
  const f = total % scale;
  total = Math.floor(total / scale);
  const s = total % 60;
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  const tail = frac ? `.${pad(f, frac)}` : "";
  const body = h ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
  return (neg ? "-" : "") + body + tail;
}

/** Короткая длительность: 1:23, 12.4 с */
export function fmtDur(t: number): string {
  if (t < 60) return `${t.toFixed(t < 10 ? 1 : 0)} с`;
  return fmtTime(t, 0);
}

/** "1:23.5", "83.5", "01:02:03" → секунды */
export function parseTime(str: string): number | null {
  const parts = str.trim().replace(",", ".").split(":");
  if (!parts.length || parts.length > 3) return null;
  let t = 0;
  for (const p of parts) {
    const v = Number(p);
    if (!Number.isFinite(v) || v < 0) return null;
    t = t * 60 + v;
  }
  return t;
}

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} Б`;
  const u = ["КБ", "МБ", "ГБ", "ТБ"];
  let i = -1;
  do {
    n /= 1024;
    i++;
  } while (n >= 1024 && i < u.length - 1);
  return `${n.toFixed(n < 10 ? 1 : 0)} ${u[i]}`;
}

/** Шаг подписей линейки времени. */
export function niceStep(span: number, maxLabels: number): number {
  const raw = span / Math.max(1, maxLabels);
  const steps = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200];
  return steps.find((s) => s >= raw) ?? 14400;
}

export function fmtRuler(t: number, step: number): string {
  return fmtTime(t, step < 1 ? 1 : 0);
}

export const clamp = (v: number, a: number, b: number) => Math.min(b, Math.max(a, v));

export function codecLabel(c: string): string {
  return (
    { h264: "H.264", hevc: "HEVC", av1: "AV1", vp9: "VP9", vp8: "VP8", prores: "ProRes", mpeg4: "MPEG-4" } as Record<string, string>
  )[c] ?? c.toUpperCase();
}
