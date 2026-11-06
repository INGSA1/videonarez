<script lang="ts">
  import { app, seek, setIn, setOut, curRange, selectSegment, addSegmentAround, loadWaveform } from "../lib/store.svelte";
  import { fmtTime, fmtRuler, niceStep, clamp } from "../lib/format";
  import Icon from "./Icon.svelte";

  // Геометрия дорожек
  const RULER = 22;
  const THUMB = 50;
  const GAP = 3;
  const WAVE = 46;
  const LANE = 18;
  const TOP_T = RULER;
  const TOP_W = RULER + THUMB + GAP;
  const TOP_L = TOP_W + WAVE + GAP;
  const H = TOP_L + LANE + 6;
  const BODY_H = THUMB + GAP + WAVE;

  // Та же палитра, что и в app.css: графит, мел, жёлтые метки монтажа.
  const C = {
    bg: "#1b1c1e",
    ruler: "#232427",
    tick: "#3d4045",
    tickText: "#7d8086",
    track: "#202124",
    wave: "#55585e",
    waveIn: "#c9c7c2",
    accent: "#f5c518",
    marker: "230,228,223",
    seg: "#4a5a6e",
    segSel: "#f5c518",
    segText: "#e6e4df",
    segTextSel: "#1b1c1e",
    playhead: "#ffffff",
  };
  const FONT_UI = '11px "Golos Text", "Segoe UI", sans-serif';
  const FONT_TC = '500 10px "Unbounded", "Golos Text", sans-serif';

  let wrap: HTMLDivElement;
  let base: HTMLCanvasElement;
  let over: HTMLCanvasElement;
  let width = $state(0);
  let imgTick = $state(0);
  let hoverX = $state(-1);
  let cursor = $state("default");

  const images = new Map<string, HTMLImageElement>();
  $effect(() => {
    if (app.thumbs.length === 0) images.clear();
    for (const t of app.thumbs) {
      if (images.has(t.data)) continue;
      const im = new Image();
      im.onload = () => imgTick++;
      im.src = t.data;
      images.set(t.data, im);
    }
  });

  $effect(() => {
    const ro = new ResizeObserver(([e]) => (width = Math.floor(e.contentRect.width)));
    ro.observe(wrap);
    return () => ro.disconnect();
  });

  const span = () => Math.max(1e-6, app.view.end - app.view.start);
  const t2x = (t: number) => ((t - app.view.start) / span()) * width;
  const x2t = (x: number) => app.view.start + (x / Math.max(1, width)) * span();

  function setup(c: HTMLCanvasElement) {
    const dpr = window.devicePixelRatio || 1;
    const W = Math.max(1, Math.floor(width * dpr));
    const HH = Math.floor(H * dpr);
    if (c.width !== W || c.height !== HH) {
      c.width = W;
      c.height = HH;
    }
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, H);
    return ctx;
  }

  function drawBase() {
    if (!base || width <= 0) return;
    void imgTick;
    const ctx = setup(base);
    const m = app.meta;
    ctx.fillStyle = C.bg;
    ctx.fillRect(0, 0, width, H);
    if (!m) return;
    const vs = app.view.start;
    const ve = app.view.end;

    // Линейка
    ctx.fillStyle = C.ruler;
    ctx.fillRect(0, 0, width, RULER);
    const stepT = niceStep(ve - vs, width / 95);
    const minor = stepT / 5;
    ctx.fillStyle = C.tick;
    for (let t = Math.ceil(vs / minor) * minor; t <= ve; t += minor) {
      ctx.fillRect(Math.round(t2x(t)), RULER - 4, 1, 4);
    }
    ctx.font = FONT_TC;
    ctx.textBaseline = "middle";
    for (let t = Math.ceil(vs / stepT) * stepT; t <= ve; t += stepT) {
      const x = Math.round(t2x(t));
      ctx.fillStyle = C.tick;
      ctx.fillRect(x, RULER - 9, 1, 9);
      ctx.fillStyle = C.tickText;
      ctx.fillText(fmtRuler(t, stepT), x + 4, RULER / 2 - 1);
    }

    // Миниатюры (тайлы, ближайший по времени кадр)
    ctx.fillStyle = C.track;
    ctx.fillRect(0, TOP_T, width, THUMB);
    if (app.thumbs.length) {
      const thumbs = [...app.thumbs].sort((a, b) => a.time - b.time);
      const aspect = m.displayWidth && m.displayHeight ? m.displayWidth / m.displayHeight : 16 / 9;
      const tw = Math.max(28, THUMB * aspect);
      for (let x = 0; x < width; x += tw) {
        const t = x2t(x + tw / 2);
        let best = thumbs[0];
        for (const th of thumbs) if (Math.abs(th.time - t) < Math.abs(best.time - t)) best = th;
        const im = images.get(best.data);
        if (im && im.complete && im.naturalWidth) {
          ctx.drawImage(im, x, TOP_T, tw, THUMB);
        }
      }
      ctx.fillStyle = "rgba(0,0,0,0.25)";
      for (let x = 0; x < width; x += tw) ctx.fillRect(Math.round(x), TOP_T, 1, THUMB);
    }

    // Волна
    ctx.fillStyle = C.track;
    ctx.fillRect(0, TOP_W, width, WAVE);
    const r = curRange();
    const wf = app.waveform;
    if (wf && wf.peaks.length) {
      const n = wf.peaks.length;
      const per = wf.bucketSec;
      const mid = TOP_W + WAVE / 2;
      for (let x = 0; x < width; x++) {
        const ta = x2t(x);
        const a = Math.floor(ta / per);
        const b = Math.max(a + 1, Math.floor(x2t(x + 1) / per));
        let mx = 0;
        for (let i = Math.max(0, a); i < Math.min(n, b); i++) if (wf.peaks[i] > mx) mx = wf.peaks[i];
        const hh = mx * mx * (WAVE - 6) / 2; // квадрат — визуально контрастнее
        if (hh < 0.4) continue;
        ctx.fillStyle = ta >= r.start && ta <= r.end ? C.waveIn : C.wave;
        ctx.fillRect(x, mid - hh, 1, hh * 2);
      }
      for (const mk of wf.markers) {
        const x = t2x(mk.time);
        if (x < -6 || x > width + 6) continue;
        const a = 0.35 + 0.65 * mk.score;
        ctx.fillStyle = `rgba(${C.marker},${a * 0.35})`;
        ctx.fillRect(x - 0.5, TOP_W, 1.5, WAVE);
        ctx.fillStyle = `rgba(${C.marker},${a})`;
        ctx.beginPath();
        ctx.moveTo(x - 5, TOP_W);
        ctx.lineTo(x + 5, TOP_W);
        ctx.lineTo(x, TOP_W + 7);
        ctx.fill();
      }
    } else {
      ctx.fillStyle = C.tickText;
      ctx.font = FONT_UI;
      const msg = app.waveLoading ? "Анализ звука…" : m.audioTracks.length ? "" : "Нет звуковой дорожки";
      if (msg) ctx.fillText(msg, 10, TOP_W + WAVE / 2);
    }

    // Дорожка клипов
    ctx.fillStyle = C.track;
    ctx.fillRect(0, TOP_L, width, LANE);
    ctx.font = FONT_UI;
    app.segments.forEach((s, i) => {
      const x1 = t2x(s.start);
      const x2 = t2x(s.end);
      if (x2 < 0 || x1 > width) return;
      const sel = s.id === app.selectedId;
      ctx.fillStyle = sel ? C.segSel : C.seg;
      ctx.beginPath();
      ctx.roundRect(x1, TOP_L + 2, Math.max(3, x2 - x1), LANE - 4, 4);
      ctx.fill();
      const label = s.name || `#${i + 1}`;
      if (x2 - x1 > 22) {
        ctx.save();
        ctx.beginPath();
        ctx.rect(x1, TOP_L, x2 - x1 - 4, LANE);
        ctx.clip();
        ctx.fillStyle = sel ? C.segTextSel : C.segText;
        ctx.fillText(label, Math.max(x1, 0) + 6, TOP_L + LANE / 2 + 1);
        ctx.restore();
      }
    });

    // Затемнение вне выделения + рамка с ручками
    const xi = t2x(r.start);
    const xo = t2x(r.end);
    ctx.fillStyle = "rgba(17,18,20,0.62)";
    if (xi > 0) ctx.fillRect(0, TOP_T, Math.min(xi, width), BODY_H);
    if (xo < width) ctx.fillRect(Math.max(0, xo), TOP_T, width - Math.max(0, xo), BODY_H);
    ctx.fillStyle = "rgba(245,197,24,0.6)";
    ctx.fillRect(xi, RULER - 3, xo - xi, 3);
    ctx.strokeStyle = C.accent;
    ctx.lineWidth = 2;
    ctx.strokeRect(xi, TOP_T + 1, xo - xi, BODY_H - 2);
    for (const [hx, dir] of [[xi, -1], [xo, 1]] as const) {
      const x = dir < 0 ? hx - 9 : hx;
      ctx.fillStyle = C.accent;
      ctx.beginPath();
      ctx.roundRect(x, TOP_T, 9, BODY_H, dir < 0 ? [5, 0, 0, 5] : [0, 5, 5, 0]);
      ctx.fill();
      ctx.fillStyle = "rgba(0,0,0,0.45)";
      ctx.fillRect(x + 3, TOP_T + BODY_H / 2 - 8, 1, 16);
      ctx.fillRect(x + 5, TOP_T + BODY_H / 2 - 8, 1, 16);
    }
  }

  function drawOver() {
    if (!over || width <= 0) return;
    const ctx = setup(over);
    if (!app.meta) return;
    if (hoverX >= 0) {
      ctx.fillStyle = "rgba(255,255,255,0.22)";
      ctx.fillRect(hoverX, RULER, 1, H - RULER);
    }
    const x = t2x(app.currentTime);
    if (x >= -2 && x <= width + 2) {
      ctx.fillStyle = C.playhead;
      ctx.fillRect(Math.round(x) - 1, 0, 2, H);
      ctx.beginPath();
      ctx.moveTo(x - 6, 0);
      ctx.lineTo(x + 6, 0);
      ctx.lineTo(x, 8);
      ctx.fill();
    }
    if (hoverX >= 0) {
      const label = fmtTime(x2t(hoverX));
      ctx.font = FONT_TC;
      const w = ctx.measureText(label).width + 10;
      const lx = clamp(hoverX - w / 2, 0, width - w);
      ctx.fillStyle = "#e6e4df";
      ctx.beginPath();
      ctx.roundRect(lx, 2, w, 17, 4);
      ctx.fill();
      ctx.fillStyle = "#1b1c1e";
      ctx.textBaseline = "middle";
      ctx.fillText(label, lx + 5, 11);
    }
  }

  $effect(() => drawBase());
  $effect(() => drawOver());

  // Автопрокрутка вида за курсором воспроизведения
  $effect(() => {
    const t = app.currentTime;
    if (!app.playing || !app.meta) return;
    const s = app.view.end - app.view.start;
    if (t > app.view.end || t < app.view.start) {
      const start = clamp(t - s * 0.1, 0, Math.max(0, app.meta.duration - s));
      app.view = { start, end: start + s };
    }
  });

  // ---------- Взаимодействие ----------

  type Drag = { kind: "in" | "out" | "scrub" | "pan"; x0: number; v0: { start: number; end: number } } | null;
  let drag: Drag = null;

  function pos(e: PointerEvent | MouseEvent | WheelEvent) {
    const r = over.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function hitHandle(x: number, y: number): "in" | "out" | null {
    if (y < TOP_T || y > TOP_T + BODY_H) return null;
    const r = curRange();
    const xi = t2x(r.start);
    const xo = t2x(r.end);
    if (x >= xi - 11 && x <= xi + 3) return "in";
    if (x >= xo - 3 && x <= xo + 11) return "out";
    return null;
  }

  function markerAt(x: number, y: number) {
    if (y < TOP_W || y > TOP_W + WAVE || !app.waveform) return null;
    return app.waveform.markers.find((mk) => Math.abs(t2x(mk.time) - x) <= 6) ?? null;
  }

  function segmentAt(x: number, y: number) {
    if (y < TOP_L || y > TOP_L + LANE) return undefined;
    const t = x2t(x);
    return app.segments.find((s) => t >= s.start && t <= s.end);
  }

  function onDown(e: PointerEvent) {
    if (!app.meta) return;
    const { x, y } = pos(e);
    over.setPointerCapture(e.pointerId);
    const v0 = { ...app.view };
    if (e.button === 1 || e.button === 2) {
      drag = { kind: "pan", x0: x, v0 };
      return;
    }
    const h = hitHandle(x, y);
    if (h) {
      drag = { kind: h, x0: x, v0 };
      return;
    }
    if (y >= TOP_L) {
      const s = segmentAt(x, y);
      if (s) {
        selectSegment(s.id);
        seek(s.start);
      } else app.selectedId = null;
      return;
    }
    const mk = markerAt(x, y);
    if (mk) {
      seek(mk.time);
      return;
    }
    drag = { kind: "scrub", x0: x, v0 };
    seek(x2t(x));
  }

  function onMove(e: PointerEvent) {
    if (!app.meta) return;
    const { x, y } = pos(e);
    hoverX = x;
    if (!drag) {
      cursor = hitHandle(x, y) ? "ew-resize" : markerAt(x, y) || segmentAt(x, y) ? "pointer" : "default";
      return;
    }
    const t = x2t(clamp(x, 0, width));
    if (drag.kind === "in") setIn(t);
    else if (drag.kind === "out") setOut(t);
    else if (drag.kind === "scrub") seek(t);
    else {
      const s = drag.v0.end - drag.v0.start;
      const dt = ((x - drag.x0) / width) * s;
      const start = clamp(drag.v0.start - dt, 0, app.meta.duration - s);
      app.view = { start, end: start + s };
    }
  }

  function onUp() {
    drag = null;
  }

  function zoomAt(x: number, factor: number) {
    const m = app.meta;
    if (!m) return;
    const t = x2t(x);
    const minSpan = Math.min(m.duration, Math.max(0.5, 20 / (m.fps || 30)));
    const s = clamp(span() * factor, minSpan, m.duration);
    let start = t - (x / width) * s;
    start = clamp(start, 0, m.duration - s);
    app.view = { start, end: start + s };
  }

  export function zoom(factor: number) {
    zoomAt(clamp(t2x(app.currentTime), 0, width), factor);
  }
  export function fit() {
    if (app.meta) app.view = { start: 0, end: app.meta.duration };
  }

  $effect(() => {
    const onWheel = (e: WheelEvent) => {
      if (!app.meta) return;
      e.preventDefault();
      const { x } = pos(e);
      if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        const d = (e.shiftKey ? e.deltaY : e.deltaX) / width;
        const s = span();
        const start = clamp(app.view.start + d * s, 0, app.meta.duration - s);
        app.view = { start, end: start + s };
      } else {
        zoomAt(x, Math.exp(e.deltaY * 0.0015));
      }
    };
    over.addEventListener("wheel", onWheel, { passive: false });
    return () => over.removeEventListener("wheel", onWheel);
  });

  function onDbl(e: MouseEvent) {
    const { x, y } = pos(e);
    const mk = markerAt(x, y);
    if (mk) addSegmentAround(mk.time);
    else if (y < RULER) fit();
  }
</script>

<div class="timeline">
  <div class="bar">
    <div class="legend">
      <span class="dim">Колесо мыши меняет масштаб, с Shift прокручивает. Двойной щелчок по метке громкости создаёт клип вокруг неё.</span>
    </div>
    <div class="tools">
      {#if app.meta && app.meta.audioTracks.length > 1}
        <select value={app.waveTrack} onchange={(e) => loadWaveform(Number((e.target as HTMLSelectElement).value))} title="Дорожка для волны и пиков">
          {#each app.meta.audioTracks as t (t.index)}
            <option value={t.index}>{t.title || `Звук ${t.index + 1}`}</option>
          {/each}
        </select>
      {/if}
      <button class="ghost icon" title="Уменьшить (−)" onclick={() => zoom(1.5)}><Icon name="zoomOut" /></button>
      <button class="ghost icon" title="Увеличить (+)" onclick={() => zoom(1 / 1.5)}><Icon name="zoomIn" /></button>
      <button class="ghost icon" title="Весь ролик (0)" onclick={fit}><Icon name="fit" /></button>
    </div>
  </div>
  <div class="canvas-wrap" bind:this={wrap} style="height:{H}px">
    <canvas bind:this={base} style="width:{width}px;height:{H}px"></canvas>
    <canvas
      bind:this={over}
      class="over"
      style="width:{width}px;height:{H}px;cursor:{cursor}"
      onpointerdown={onDown}
      onpointermove={onMove}
      onpointerup={onUp}
      onpointerleave={() => (hoverX = -1)}
      ondblclick={onDbl}
      oncontextmenu={(e) => e.preventDefault()}
    ></canvas>
  </div>
</div>

<style>
  .timeline {
    background: var(--panel);
    border-top: 1px solid var(--line);
  }
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 10px;
    font-size: 11px;
    gap: 10px;
  }
  .legend {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .tools select {
    font-size: 11px;
    padding: 2px 6px;
    margin-right: 6px;
  }
  .canvas-wrap {
    position: relative;
    margin: 0 10px 10px;
  }
  canvas {
    position: absolute;
    inset: 0;
    border-radius: 6px;
  }
  .over {
    touch-action: none;
  }
</style>
