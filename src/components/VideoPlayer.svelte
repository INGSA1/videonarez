<script lang="ts">
  import { untrack } from "svelte";
  import { app, player, curRange, onVideoError, togglePlay, step, skip, setIn, setOut, playRange, addSegment, seek, videoCrop } from "../lib/store.svelte";
  import { FRAME_ASPECTS, FRAME_DEFAULTS, MAX_CROP_ZOOM } from "../lib/presets";
  import { fmtTime, clamp } from "../lib/format";
  import Icon from "./Icon.svelte";
  import Timecode from "./Timecode.svelte";
  import { Mixer } from "../lib/mixer";

  let video: HTMLVideoElement;
  let stage: HTMLDivElement;
  let sw = $state(0);
  let sh = $state(0);
  let videoWidth = $state(0);
  let videoHeight = $state(0);
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

  // Несколько звуковых дорожек: видео немое, звучит микшер (каждая дорожка со своей громкостью).
  let mixer: Mixer | null = null;
  $effect(() => {
    const urls = app.trackUrls;
    if (urls.length < 2) return;
    const m = new Mixer(urls);
    mixer = m;
    if (video && !video.paused) m.play(video.currentTime);
    return () => {
      m.destroy();
      if (mixer === m) mixer = null;
    };
  });

  $effect(() => {
    const multi = app.trackUrls.length > 1;
    if (video) {
      video.volume = volume;
      video.muted = muted || multi;
      video.playbackRate = rate;
    }
    if (multi && mixer) {
      mixer.setMaster(volume, muted);
      mixer.setRate(rate);
      for (const [i, o] of Object.entries(app.opts.tracks)) mixer.setTrack(Number(i), o.enabled, o.volume);
    }
  });

  function fitRect(width: number, height: number) {
    const scale = Math.min(sw / width, sh / height);
    const w = width * scale;
    const h = height * scale;
    return { x: (sw - w) / 2, y: (sh - h) / 2, w, h };
  }

  // Прямоугольник, который реально занимает видео внутри сцены (object-fit: contain).
  const vr = $derived.by(() => {
    const m = app.meta;
    if (!m?.displayWidth || !m.displayHeight) return { x: 0, y: 0, w: 0, h: 0 };
    return fitRect(videoWidth || m.displayWidth, videoHeight || m.displayHeight);
  });

  const crop = $derived.by(() => {
    const m = app.meta;
    const r = videoCrop();
    if (!m || !r || !vr.w || !vr.h) return null;
    const sx = vr.w / m.displayWidth;
    const sy = vr.h / m.displayHeight;
    return { x: vr.x + r.x * sx, y: vr.y + r.y * sy, w: r.w * sx, h: r.h * sy, src: r };
  });

  const preview = $derived.by(() => {
    if (app.editingFrame || !crop) return null;
    const { w, h } = crop.src.frameSize;
    const view = fitRect(w, h);
    const sx = view.w / crop.w;
    const sy = view.h / crop.h;
    const x = view.x - crop.x * sx;
    const y = view.y - crop.y * sy;
    return {
      clip: `inset(${view.y}px ${view.x}px)`,
      transform: `translate(${x}px, ${y}px) scale(${sx}, ${sy})`,
    };
  });

  // Обновление времени покадрово только во время воспроизведения.
  function loop() {
    if (!video) return;
    app.currentTime = video.currentTime;
    mixer?.sync(video.currentTime);
    {
      // Воспроизведение не выходит за выделение: на его конце останавливаемся.
      const r = curRange();
      if (video.currentTime >= r.end) {
        video.pause();
        app.playRange = false;
        video.currentTime = r.end;
        app.currentTime = r.end;
      } else if (video.currentTime < r.start - 0.05) {
        video.currentTime = r.start;
        app.currentTime = r.start;
      }
    }
    if (!video.paused) raf = requestAnimationFrame(loop);
  }

  function onplay() {
    app.playing = true;
    mixer?.play(video.currentTime);
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(loop);
  }
  function onpause() {
    app.playing = false;
    mixer?.pause(video.currentTime);
    cancelAnimationFrame(raf);
    app.currentTime = video.currentTime;
  }
  function onseeked() {
    mixer?.seek(video.currentTime);
    if (video.paused) app.currentTime = video.currentTime;
  }

  // Перетаскивание рамки кадрирования
  let drag: { pointer: number; px: number; py: number; cx: number; cy: number } | null = null;
  function cropDown(e: PointerEvent) {
    if (e.button !== 0 || drag) return;
    drag = { pointer: e.pointerId, px: e.clientX, py: e.clientY, cx: app.opts.cropX, cy: app.opts.cropY };
    (e.currentTarget as HTMLButtonElement).setPointerCapture(e.pointerId);
  }
  function cropMove(e: PointerEvent) {
    if (!drag || e.pointerId !== drag.pointer || !crop) return;
    const freeX = vr.w - crop.w;
    const freeY = vr.h - crop.h;
    if (freeX > 0) app.opts.cropX = clamp(drag.cx + (e.clientX - drag.px) / freeX, 0, 1);
    if (freeY > 0) app.opts.cropY = clamp(drag.cy + (e.clientY - drag.py) / freeY, 0, 1);
  }
  function cropUp(e: PointerEvent) {
    if (drag?.pointer === e.pointerId) drag = null;
  }

  function cropKey(e: KeyboardEvent) {
    const step = e.shiftKey ? 0.1 : 0.02;
    switch (e.key) {
      case "ArrowLeft": app.opts.cropX = clamp(app.opts.cropX - step, 0, 1); break;
      case "ArrowRight": app.opts.cropX = clamp(app.opts.cropX + step, 0, 1); break;
      case "ArrowUp": app.opts.cropY = clamp(app.opts.cropY - step, 0, 1); break;
      case "ArrowDown": app.opts.cropY = clamp(app.opts.cropY + step, 0, 1); break;
      default: return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  function setAspect() {
    app.opts.preset = "custom";
  }

  function resetFrame() {
    Object.assign(app.opts, FRAME_DEFAULTS);
    setAspect();
  }

  const range = $derived(curRange());

  // Выделение поменялось (выбрали другой клип, сдвинули границу) — курсор возвращается внутрь.
  $effect(() => {
    const r = range;
    const t = untrack(() => app.currentTime);
    if (app.meta && (t < r.start || t > r.end)) untrack(() => seek(t));
  });
</script>

<div class="player">
  <div class="stage" bind:this={stage} ondblclick={togglePlay} role="presentation">
    <div class="picture" style:clip-path={preview?.clip}>
      <video
        bind:this={video}
        src={app.src || undefined}
        style:transform={preview?.transform}
        preload="metadata"
        playsinline
        disablepictureinpicture
        {onplay}
        {onpause}
        {onseeked}
        onresize={() => { videoWidth = video.videoWidth; videoHeight = video.videoHeight; }}
        onerror={() => app.src && onVideoError()}
        onclick={togglePlay}
      ></video>
    </div>

    {#if app.editingFrame && crop}
      <div class="shade" style="left:{vr.x}px;top:{vr.y}px;width:{vr.w}px;height:{vr.h}px;
        clip-path: polygon(0 0,100% 0,100% 100%,0 100%,0 0,{crop.x - vr.x}px {crop.y - vr.y}px,{crop.x - vr.x}px {crop.y - vr.y + crop.h}px,{crop.x - vr.x + crop.w}px {crop.y - vr.y + crop.h}px,{crop.x - vr.x + crop.w}px {crop.y - vr.y}px,{crop.x - vr.x}px {crop.y - vr.y}px)"></div>
      <button
        type="button"
        class="crop"
        title="Перетащите рамку или двигайте её стрелками"
        aria-label="Положение рамки кадрирования"
        style="left:{crop.x}px;top:{crop.y}px;width:{crop.w}px;height:{crop.h}px"
        onpointerdown={cropDown}
        onpointermove={cropMove}
        onlostpointercapture={cropUp}
        onkeydown={cropKey}
        ondblclick={(e) => e.stopPropagation()}
      >
        <span class="crop-label">{crop.src.w}×{crop.src.h}</span>
        <i class="g1"></i><i class="g2"></i><i class="g3"></i><i class="g4"></i>
      </button>
    {/if}

    {#if app.meta?.hasVideo}
      <button
        class="frame-toggle"
        class:active={app.editingFrame}
        aria-expanded={app.editingFrame}
        onclick={() => (app.editingFrame = !app.editingFrame)}
        ondblclick={(e) => e.stopPropagation()}
      >
        <Icon name={app.editingFrame ? "check" : "crop"} /> {app.editingFrame ? "Готово" : "Кадрирование"}
      </button>
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

  {#if app.editingFrame}
    <div class="frame-controls">
      <label>
        Формат
        <select bind:value={app.opts.aspect} onchange={setAspect}>
          {#each FRAME_ASPECTS as aspect (aspect)}
            <option value={aspect}>{aspect === "none" ? "Исходный" : aspect}</option>
          {/each}
        </select>
      </label>
      <label class="zoom">
        Масштаб
        <input type="range" min="100" max={MAX_CROP_ZOOM} step="1" bind:value={app.opts.cropZoom}
          aria-valuetext={`${app.opts.cropZoom}%`} />
        <span class="mono">{app.opts.cropZoom}%</span>
      </label>
      <button class="ghost" onclick={resetFrame}>Сбросить</button>
    </div>
  {/if}

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
    transform-origin: top left;
  }
  .picture {
    position: absolute;
    inset: 0;
  }
  .frame-toggle {
    position: absolute;
    top: 10px;
    right: 10px;
  }
  .frame-controls {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 6px 14px;
    background: var(--panel);
    border-top: 1px solid var(--line);
  }
  .frame-controls label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .zoom input {
    width: clamp(80px, 15vw, 180px);
  }
  .zoom span {
    min-width: 4ch;
  }
  .shade {
    position: absolute;
    background: rgba(5, 5, 8, 0.66);
    pointer-events: none;
  }
  .crop {
    position: absolute;
    border: 2px solid var(--cue);
    border-radius: 0;
    background: transparent;
    padding: 0;
    cursor: move;
    touch-action: none;
  }
  .crop:hover {
    background: transparent;
    border-color: var(--cue);
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
    pointer-events: none;
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
