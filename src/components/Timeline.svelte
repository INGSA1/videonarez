<script lang="ts">
  import { app, seek, setIn, setOut, curRange, selectSegment, addSegmentAround, loadWaveform } from "../lib/store.svelte";
  import { fmtTime, fmtRuler, niceStep, clamp } from "../lib/format";
  import Icon from "./Icon.svelte";

  // Геометрия дорожек
  const RULER = 22;
  const THUMB = 50;
  const GAP = 3;
  const LANE = 18;
  const HANDLE = 14;
  const TOP_T = RULER;
  const TOP_W = RULER + THUMB + GAP;
  // Звук: одна дорожка — общая волна; несколько — по полосе на каждую.
  const AL = 30;
  const AGAP = 2;
  const nTracks = $derived(Math.max(1, app.meta?.audioTracks.length ?? 1));
  const WAVE = $derived(nTracks > 1 ? nTracks * AL + (nTracks - 1) * AGAP : 46);
  const TOP_L = $derived(TOP_W + WAVE + GAP);
  const H = $derived(TOP_L + LANE + 6);
  const BODY_H = $derived(THUMB + GAP + WAVE);

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

    // Звуковые дорожки
    const r = curRange();
    const multi = nTracks > 1;
    const laneH = multi ? AL : WAVE;
    for (let k = 0; k < nTracks; k++) {
      const y0 = TOP_W + k * (laneH + AGAP);
      ctx.fillStyle = C.track;
      ctx.fillRect(0, y0, width, laneH);
      const wf = multi ? app.waveforms[k] : app.waveform;
      const opt = app.opts.tracks[k];
      const on = !opt || opt.enabled;
      const gain = Math.min(2, opt?.volume ?? 1);
      if (wf && wf.peaks.length) {
        const n = wf.peaks.length;
        const per = wf.bucketSec;
        const mid = y0 + laneH / 2;
        ctx.globalAlpha = on ? 1 : 0.28;
        for (let x = 0; x < width; x++) {
          const ta = x2t(x);
          const a = Math.floor(ta / per);
          const b = Math.max(a + 1, Math.floor(x2t(x + 1) / per));
          let mx = 0;
          for (let i = Math.max(0, a); i < Math.min(n, b); i++) if (wf.peaks[i] > mx) mx = wf.peaks[i];
          // квадрат — визуально контрастнее; громкость дорожки меняет высоту волны
          const hh = Math.min(laneH / 2 - 2, (mx * mx * (laneH - 6)) / 2 * (multi ? gain : 1));
          if (hh < 0.4) continue;
          ctx.fillStyle = ta >= r.start && ta <= r.end ? C.waveIn : C.wave;
          ctx.fillRect(x, mid - hh, 1, hh * 2);
        }
        ctx.globalAlpha = 1;
      }
      if (multi) {
        const t = m.audioTracks[k];
        const label = `${t?.title || `Дорожка ${k + 1}`}${on ? "" : ", выключена"}`;
        ctx.font = FONT_UI;
        const w = ctx.measureText(label).width + 10;
        ctx.fillStyle = "rgba(27,28,30,0.85)";
        // отступ слева, чтобы подпись не пряталась под ручкой начала
        ctx.fillRect(HANDLE + 6, y0 + 3, w, 15);
        ctx.fillStyle = on ? C.segText : C.tickText;
        ctx.textBaseline = "middle";
        ctx.fillText(label, HANDLE + 11, y0 + 11);
      }
    }
    const wf = app.waveform;
    if (wf && wf.peaks.length) {
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
      const loading = app.waveLoading && !Object.keys(app.waveforms).length;
      const msg = loading ? "Анализ звука…" : m.audioTracks.length ? "" : "Нет звуковой дорожки";
      if (msg) ctx.fillText(msg, multi ? 160 : 10, TOP_W + WAVE / 2);
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
    ctx.lineWidth = 3;
    ctx.strokeRect(xi + 1.5, TOP_T + 1.5, Math.max(0, xo - xi - 3), BODY_H - 3);
    // Ручки рисуются внутри выделения: у краёв ленты (весь ролик выделен) они раньше
    // уходили за край холста и были не видны.
    const hw = Math.max(4, Math.min(HANDLE, (xo - xi) / 2));
    for (const [hx, dir] of [[xi, -1], [xo, 1]] as const) {
      const x = dir < 0 ? hx : hx - hw;
      ctx.fillStyle = C.accent;
      ctx.beginPath();
      ctx.roundRect(x, TOP_T, hw, BODY_H, dir < 0 ? [6, 0, 0, 6] : [0, 6, 6, 0]);
      ctx.fill();
      // Насечки-«хват» и стрелка, куда тянуть
      ctx.fillStyle = "rgba(27,28,30,0.7)";
      const cx = x + hw / 2;
      if (hw >= 10) {
        ctx.fillRect(Math.round(cx) - 2, TOP_T + BODY_H / 2 - 9, 1.5, 18);
        ctx.fillRect(Math.round(cx) + 1, TOP_T + BODY_H / 2 - 9, 1.5, 18);
        ctx.beginPath();
        const ay = TOP_T + 10;
        if (dir < 0) {
          ctx.moveTo(cx + 3, ay - 4);
          ctx.lineTo(cx - 2, ay);
          ctx.lineTo(cx + 3, ay + 4);
        } else {
          ctx.moveTo(cx - 3, ay - 4);
          ctx.lineTo(cx + 2, ay);
          ctx.lineTo(cx - 3, ay + 4);
        }
        ctx.fill();
      }
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
    const hw = Math.max(4, Math.min(HANDLE, (xo - xi) / 2));
    if (x >= xi - 6 && x <= xi + hw + 2) return "in";
    if (x >= xo - hw - 2 && x <= xo + 6) return "out";
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
    // Тянем границу — курсор едет вместе с ней, в плеере виден кадр, по которому режем.
    if (drag.kind === "in") {
      setIn(t);
      seek(curRange().start);
    } else if (drag.kind === "out") {
      setOut(t);
      seek(curRange().end);
    }
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
        <select value={app.waveTrack} onchange={(e) => loadWaveform(Number((e.target as HTMLSelectElement).value))} title="С какой дорожки брать метки громких моментов">
          {#each app.meta.audioTracks as t (t.index)}
            <option value={t.index}>Метки по: {t.title || `дорожка ${t.index + 1}`}</option>
          {/each}
        </select>
      {/if}
      <button class="ghost icon" title="Уменьшить (−)" onclick={() => zoom(1.5)}><Icon name="zoomOut" /></button>
      <button class="ghost icon" title="Увеличить (+)" onclick={() => zoom(1 / 1.5)}><Icon name="zoomIn" /></button>
      <button class="ghost icon" title="Весь ролик (0)" onclick={fit}><Icon name="fit" /></button>
    </div>
  </div>
  {#if app.meta && app.meta.audioTracks.length > 1}
    <div class="mixer" aria-label="Звуковые дорожки">
      {#each app.meta.audioTracks as t (t.index)}
        {@const o = app.opts.tracks[t.index]}
        {#if o}
          <div class="ch" class:off={!o.enabled}>
            <button
              class="tog"
              class:on={o.enabled}
              title={o.enabled ? "Выключить дорожку" : "Включить дорожку"}
              onclick={() => (o.enabled = !o.enabled)}
            >
              <Icon name={o.enabled ? "volume" : "mute"} size={13} />
              <span class="nm">{t.title || `Дорожка ${t.index + 1}`}</span>
            </button>
            <input type="range" min="0" max="2" step="0.05" bind:value={o.volume} disabled={!o.enabled} title="Громкость дорожки" />
            <span class="pct mono">{Math.round(o.volume * 100)}%</span>
          </div>
        {/if}
      {/each}
      {#if !app.trackUrls.length}<span class="dim prep">Готовлю дорожки для прослушивания</span>{/if}
    </div>
  {/if}
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
  .mixer {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 18px;
    align-items: center;
    padding: 2px 10px 8px;
  }
  .ch {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .tog {
    padding: 4px 8px;
    background: var(--bg);
    color: var(--text-3);
    max-width: 180px;
  }
  .tog.on {
    color: var(--text);
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ch input[type="range"] {
    width: 90px;
  }
  .ch.off input[type="range"] {
    opacity: 0.4;
  }
  .pct {
    width: 38px;
    font-size: 12px;
    color: var(--text-2);
  }
  .prep {
    font-size: 12px;
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
