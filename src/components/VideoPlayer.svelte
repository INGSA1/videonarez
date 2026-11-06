<script lang="ts">
  import { app, player, curRange, onVideoError, togglePlay, step, skip, setIn, setOut, playRange, addSegment } from "../lib/store.svelte";
  import { cropRect } from "../lib/presets";
  import { fmtTime, clamp } from "../lib/format";
  import Icon from "./Icon.svelte";
  import Timecode from "./Timecode.svelte";

  let video: HTMLVideoElement;
  let stage: HTMLDivElement;
  let sw = $state(0);
  let sh = $state(0);
  let volume = $state(1);
  let muted = $state(false);
  let rate = $state(1);
  let raf = 0;

  $effect(() => {
    player.el = video;
    return () => (player.el = null);
  });

  $effect(() => {
    const ro = new ResizeObserver(([e]) => {
      sw = e.contentRect.width;
      sh = e.contentRect.height;
    });
    ro.observe(stage);
    return () => ro.disconnect();
  });

  $effect(() => {
    if (video) {
      video.volume = volume;
      video.muted = muted;
      video.playbackRate = rate;
    }
  });

  // Прямоугольник, который реально занимает видео внутри сцены (object-fit: contain).
  const vr = $derived.by(() => {
    const m = app.meta;
    if (!m || !sw || !sh || !m.displayWidth) return { x: 0, y: 0, w: 0, h: 0 };
    const a = m.displayWidth / m.displayHeight;
    let w = sw;
    let h = sw / a;
    if (h > sh) {
      h = sh;
      w = sh * a;
    }
    return { x: (sw - w) / 2, y: (sh - h) / 2, w, h };
  });

  const crop = $derived.by(() => {
    const m = app.meta;
    if (!m || app.opts.mode !== "encode") return null;
    const r = cropRect(m.displayWidth, m.displayHeight, app.opts.aspect, app.opts.cropX, app.opts.cropY);
    if (!r) return null;
    const k = vr.w / m.displayWidth;
    return { x: vr.x + r.x * k, y: vr.y + r.y * k, w: r.w * k, h: r.h * k, src: r };
  });

  // Обновление времени покадрово только во время воспроизведения.
  function loop() {
    if (!video) return;
    app.currentTime = video.currentTime;
    if (app.playRange) {
      const r = curRange();
      if (video.currentTime >= r.end) {
        video.pause();
        app.playRange = false;
        video.currentTime = r.end;
        app.currentTime = r.end;
      }
    }
    if (!video.paused) raf = requestAnimationFrame(loop);
  }

  function onplay() {
    app.playing = true;
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(loop);
  }
  function onpause() {
    app.playing = false;
    cancelAnimationFrame(raf);
    app.currentTime = video.currentTime;
  }
  function onseeked() {
    if (video.paused) app.currentTime = video.currentTime;
  }

  // Перетаскивание рамки кадрирования
  let drag: { px: number; py: number; cx: number; cy: number } | null = null;
  function cropDown(e: PointerEvent) {
    drag = { px: e.clientX, py: e.clientY, cx: app.opts.cropX, cy: app.opts.cropY };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function cropMove(e: PointerEvent) {
    if (!drag || !crop) return;
    const freeX = vr.w - crop.w;
    const freeY = vr.h - crop.h;
    if (freeX > 1) app.opts.cropX = clamp(drag.cx + (e.clientX - drag.px) / freeX, 0, 1);
    if (freeY > 1) app.opts.cropY = clamp(drag.cy + (e.clientY - drag.py) / freeY, 0, 1);
  }
  function cropUp() {
    drag = null;
  }

  const range = $derived(curRange());
</script>

<div class="player">
  <div class="stage" bind:this={stage} ondblclick={togglePlay} role="presentation">
    <video
      bind:this={video}
      src={app.src || undefined}
      preload="metadata"
      playsinline
      disablepictureinpicture
      {onplay}
      {onpause}
      {onseeked}
      onerror={() => app.src && onVideoError()}
      onclick={togglePlay}
    ></video>

    {#if crop}
      <div class="shade" style="left:{vr.x}px;top:{vr.y}px;width:{vr.w}px;height:{vr.h}px;
        clip-path: polygon(0 0,100% 0,100% 100%,0 100%,0 0,{crop.x - vr.x}px {crop.y - vr.y}px,{crop.x - vr.x}px {crop.y - vr.y + crop.h}px,{crop.x - vr.x + crop.w}px {crop.y - vr.y + crop.h}px,{crop.x - vr.x + crop.w}px {crop.y - vr.y}px,{crop.x - vr.x}px {crop.y - vr.y}px)"></div>
      <div
        class="crop"
        style="left:{crop.x}px;top:{crop.y}px;width:{crop.w}px;height:{crop.h}px"
        onpointerdown={cropDown}
        onpointermove={cropMove}
        onpointerup={cropUp}
        role="presentation"
      >
        <span class="crop-label">{app.opts.aspect}, {crop.src.w}×{crop.src.h}</span>
        <i class="g1"></i><i class="g2"></i><i class="g3"></i><i class="g4"></i>
      </div>
    {/if}

    {#if app.proxyPercent >= 0}
      <div class="overlay">
        <div class="ov-box">
          <div>Готовлю предпросмотр, {app.proxyPercent.toFixed(0)}%</div>
          <div class="bar"><span style="width:{app.proxyPercent}%"></span></div>
          <div class="dim">Этот формат не воспроизводится напрямую, поэтому для просмотра делается облегчённая копия. Экспорт всё равно идёт из оригинала.</div>
        </div>
      </div>
    {/if}
  </div>

  <div class="controls">
    <div class="grp">
      <button class="ghost icon" title="Назад 5 с (J)" onclick={() => skip(-5)}><Icon name="back5" /></button>
      <button class="ghost icon" title="Кадр назад (←)" onclick={() => step(-1)}><Icon name="prev" /></button>
      <button class="play" title="Пуск / пауза (Пробел)" onclick={togglePlay}>
        <Icon name={app.playing ? "pause" : "play"} size={18} />
      </button>
      <button class="ghost icon" title="Кадр вперёд (→)" onclick={() => step(1)}><Icon name="next" /></button>
      <button class="ghost icon" title="Вперёд 5 с (L)" onclick={() => skip(5)}><Icon name="fwd5" /></button>
    </div>

    <div class="time">
      <Timecode value={fmtTime(app.currentTime)} size={17} />
      <span class="total"><Timecode value={fmtTime(app.meta?.duration ?? 0)} size={11} tone="dim" /></span>
    </div>

    <div class="grp mid">
      <button class="mark" title="Поставить начало клипа (I)" onclick={() => setIn(app.currentTime)}>
        <Icon name="in" /> <Timecode value={fmtTime(range.start)} size={11} />
      </button>
      <button class="mark" title="Поставить конец клипа (O)" onclick={() => setOut(app.currentTime)}>
        <Icon name="out" /> <Timecode value={fmtTime(range.end)} size={11} />
      </button>
      <span class="len" title="Длительность выделения"><Timecode value={fmtTime(range.end - range.start, 1)} size={11} tone="cue" /></span>
      <button class="ghost icon" title="Проиграть выделенное (P)" onclick={playRange}><Icon name="loop" /></button>
      {#if app.selectedId === null}
        <button title="Добавить выделение в список клипов (Enter)" onclick={() => addSegment()}>
          <Icon name="plus" /> В список
        </button>
      {/if}
    </div>

    <div class="grp right">
      <select bind:value={rate} title="Скорость предпросмотра">
        {#each [0.25, 0.5, 1, 1.5, 2] as r (r)}<option value={r}>{r}×</option>{/each}
      </select>
      <button class="ghost icon" title="Звук (M)" onclick={() => (muted = !muted)}>
        <Icon name={muted ? "mute" : "volume"} />
      </button>
      <input type="range" min="0" max="1" step="0.01" bind:value={volume} class="vol" />
    </div>
  </div>
</div>

<style>
  .player {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
  }
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    background: #050507;
    overflow: hidden;
  }
  video {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #050507;
  }
  .shade {
    position: absolute;
    background: rgba(5, 5, 8, 0.66);
    pointer-events: none;
  }
  .crop {
    position: absolute;
    border: 2px solid var(--cue);
    cursor: move;
    touch-action: none;
  }
  .crop i {
    position: absolute;
    background: rgba(255, 255, 255, 0.18);
    pointer-events: none;
  }
  .crop .g1 { left: 33.33%; top: 0; bottom: 0; width: 1px; }
  .crop .g2 { left: 66.66%; top: 0; bottom: 0; width: 1px; }
  .crop .g3 { top: 33.33%; left: 0; right: 0; height: 1px; }
  .crop .g4 { top: 66.66%; left: 0; right: 0; height: 1px; }
  .crop-label {
    position: absolute;
    top: 6px;
    left: 6px;
    font-size: 12px;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: 3px;
    background: var(--cue);
    color: var(--cue-ink);
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(5, 5, 8, 0.75);
  }
  .ov-box {
    width: 360px;
    display: grid;
    gap: 10px;
    text-align: center;
  }
  .bar {
    height: 3px;
    background: var(--panel-3);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--text);
    transition: width 0.2s;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 8px 14px;
    background: var(--panel);
    border-top: 1px solid var(--line);
  }
  .grp {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .grp.mid {
    gap: 6px;
    margin-left: auto;
  }
  .grp.right {
    margin-left: auto;
  }
  .play {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    justify-content: center;
    padding: 0;
    background: var(--text);
    color: var(--bg);
  }
  .play:hover:not(:disabled) {
    background: #fff;
  }
  .time {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  /* Метки In/Out — жёлтые, это точки монтажа. */
  .mark {
    background: var(--bg);
    box-shadow: inset 2px 0 0 var(--cue);
  }
  .len {
    padding: 6px 4px;
  }
  .vol {
    width: 80px;
  }
</style>
