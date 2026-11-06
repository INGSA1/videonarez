<script lang="ts">
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import {
    app, init, openFileDialog, openPath, togglePlay, step, skip, setIn, setOut, addSegment,
    removeSegment, playRange, startExport, scheduleSave, player, VIDEO_EXT,
  } from "./lib/store.svelte";
  import { fmtBytes, codecLabel } from "./lib/format";
  import VideoPlayer from "./components/VideoPlayer.svelte";
  import Timeline from "./components/Timeline.svelte";
  import SegmentList from "./components/SegmentList.svelte";
  import ExportPanel from "./components/ExportPanel.svelte";
  import SettingsModal from "./components/SettingsModal.svelte";
  import CapturePanel from "./components/CapturePanel.svelte";
  import QueuePanel from "./components/QueuePanel.svelte";
  import Icon from "./components/Icon.svelte";
  import { fmtTime } from "./lib/format";

  let timeline: Timeline | undefined = $state();

  $effect(() => {
    void init();
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "over" || p.type === "enter") app.dragOver = true;
        else if (p.type === "drop") {
          app.dragOver = false;
          const file = p.paths.find((x) => VIDEO_EXT.includes(x.split(".").pop()?.toLowerCase() ?? ""));
          if (file) void openPath(file);
        } else app.dragOver = false;
      })
      .then((u) => (unlisten = u));
    return () => unlisten?.();
  });

  // Сохранять параметры экспорта и выбранный режим между запусками.
  $effect(() => {
    JSON.stringify(app.opts);
    void app.mode;
    scheduleSave();
  });

  const cst = $derived(app.cap.status);
  const activeJobs = $derived(app.queue.filter((j) => j.status === "queued" || j.status === "running").length);

  function onKey(e: KeyboardEvent) {
    const tag = (e.target as HTMLElement)?.tagName;
    const typing = tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA";
    if (e.ctrlKey && e.code === "KeyO") {
      e.preventDefault();
      void openFileDialog();
      return;
    }
    if (e.ctrlKey && e.code === "KeyE") {
      e.preventDefault();
      void startExport();
      return;
    }
    if (typing || app.showSettings || app.mode !== "editor" || !app.meta) return;
    const handled = () => e.preventDefault();
    switch (e.code) {
      case "Space": handled(); togglePlay(); break;
      case "ArrowLeft": handled(); e.shiftKey ? skip(-1) : step(-1); break;
      case "ArrowRight": handled(); e.shiftKey ? skip(1) : step(1); break;
      case "KeyJ": handled(); skip(-5); break;
      case "KeyL": handled(); skip(5); break;
      case "KeyK": handled(); player.el?.pause(); break;
      case "KeyI": handled(); setIn(app.currentTime); break;
      case "KeyO": handled(); setOut(app.currentTime); break;
      case "KeyP": handled(); playRange(); break;
      case "KeyM": handled(); if (player.el) player.el.muted = !player.el.muted; break;
      case "Enter": case "NumpadEnter": handled(); if (app.selectedId === null) addSegment(); break;
      case "Delete": if (app.selectedId !== null) { handled(); removeSegment(app.selectedId); } break;
      case "Escape": app.selectedId = null; break;
      case "Equal": case "NumpadAdd": handled(); timeline?.zoom(1 / 1.5); break;
      case "Minus": case "NumpadSubtract": handled(); timeline?.zoom(1.5); break;
      case "Digit0": case "Numpad0": handled(); timeline?.fit(); break;
      case "Home": handled(); player.el && (player.el.currentTime = 0); app.currentTime = 0; break;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="shell">
  <header class="top">
    <div class="brand">
      <span class="logo"><Icon name="scissors" size={16} /></span>
      <b>Videonarez</b>
    </div>
    <nav class="modes">
      <button class:on={app.mode === "editor"} onclick={() => (app.mode = "editor")}><Icon name="scissors" size={14} /> Редактор</button>
      <button class:on={app.mode === "capture"} onclick={() => (app.mode = "capture")}>
        <span class="cdot" class:live={cst?.running} class:wait={cst?.waitingForGame} class:rec={cst?.recordingSecs != null}></span> Захват
        {#if cst?.recordingSecs != null}<span class="mono rec-t">{fmtTime(cst.recordingSecs, 0)}</span>
        {:else if cst?.running}<span class="mono dim">{Math.floor(cst.bufferedSecs)} с</span>{/if}
      </button>
    </nav>
    {#if app.mode === "editor"}
    <button onclick={openFileDialog} title="Открыть видео (Ctrl+O)"><Icon name="open" /> Открыть</button>
    {/if}
    {#if app.meta && app.mode === "editor"}
      {@const m = app.meta}
      <div class="file">
        <span class="name" title={m.path}>{m.fileName}</span>
        <span class="chips">
          {#if m.hasVideo}<span class="chip">{m.width}×{m.height}</span><span class="chip">{+m.fps.toFixed(2)} fps</span><span class="chip">{codecLabel(m.videoCodec)}</span>{/if}
          {#if m.audioTracks.length}<span class="chip">{m.audioTracks.length} аудио</span>{/if}
          <span class="chip">{fmtBytes(m.sizeBytes)}</span>
          {#if app.usingProxy}<span class="chip warn" title="Предпросмотр идёт через облегчённую копию; экспорт — из оригинала">прокси</span>{/if}
        </span>
      </div>
    {/if}
    <div class="spacer"></div>
    <button class="ghost icon" title="Настройки" onclick={() => (app.showSettings = true)}><Icon name="gear" size={18} /></button>
  </header>

  <main>
    {#if app.mode === "capture"}
      <CapturePanel />
    {:else}
    <div class="editor">
      {#if app.meta}
        <VideoPlayer />
        <Timeline bind:this={timeline} />
      {:else}
        <div class="welcome">
          <div class="drop" class:over={app.dragOver}>
            <div class="big"><Icon name="scissors" size={40} /></div>
            <h2>Перетащите видео сюда</h2>
            <p class="muted">MP4, MKV, MOV, WebM, AVI и другие. Файл не загружается в память — читаются только нужные куски.</p>
            <button class="primary" onclick={openFileDialog} disabled={app.loading}>
              <Icon name="open" /> {app.loading ? "Открываю…" : "Выбрать файл"}
            </button>
            <p class="dim keys"><kbd>I</kbd>/<kbd>O</kbd> — начало и конец · <kbd>Enter</kbd> — клип · <kbd>Ctrl+E</kbd> — экспорт</p>
          </div>
        </div>
      {/if}
    </div>

    <aside>
      <nav class="tabs">
        <button class:on={app.tab === "clips"} onclick={() => (app.tab = "clips")}>
          Клипы {#if app.segments.length}<span class="badge">{app.segments.length}</span>{/if}
        </button>
        <button class:on={app.tab === "export"} onclick={() => (app.tab = "export")}>Экспорт</button>
        <button class:on={app.tab === "queue"} onclick={() => (app.tab = "queue")}>
          Очередь {#if activeJobs}<span class="badge">{activeJobs}</span>{/if}
        </button>
      </nav>
      <div class="side-body">
        {#if app.tab === "clips"}
          <SegmentList />
        {:else if app.tab === "export"}
          <ExportPanel />
        {:else}
          <QueuePanel />
        {/if}
      </div>
      {#if app.tab === "clips" && app.meta}
        <div class="side-foot">
          <button class="primary" onclick={() => (app.tab = "export")}>Далее: экспорт →</button>
        </div>
      {/if}
    </aside>
    {/if}
  </main>

  {#if app.dragOver && app.meta}
    <div class="dragmask"><div>Отпустите, чтобы открыть</div></div>
  {/if}

  {#if app.error}
    <div class="error">
      <span>{app.error}</span>
      <button class="ghost icon" onclick={() => (app.error = "")}><Icon name="x" /></button>
    </div>
  {/if}
  {#if app.toast}<div class="toast">{app.toast}</div>{/if}

  {#if app.showSettings}<SettingsModal />{/if}
</div>

<style>
  .shell {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
    min-height: 48px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    letter-spacing: 0.01em;
  }
  .logo {
    width: 26px;
    height: 26px;
    border-radius: 7px;
    display: grid;
    place-items: center;
    color: #fff;
    background: linear-gradient(135deg, var(--accent), var(--accent-2));
  }
  .modes {
    display: flex;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 2px;
  }
  .modes button {
    border: none;
    background: transparent;
    padding: 5px 12px;
    color: var(--text-2);
  }
  .modes button.on {
    background: var(--panel-3);
    color: var(--text);
  }
  .cdot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-3);
  }
  .cdot.live {
    background: var(--ok);
  }
  .cdot.wait {
    background: var(--warn);
  }
  .cdot.rec {
    background: var(--accent-2);
  }
  .rec-t {
    color: #ff9db5;
  }
  .file {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 360px;
  }
  .chips {
    display: flex;
    gap: 4px;
  }
  .chip {
    font-size: 11px;
    padding: 2px 7px;
    border-radius: 10px;
    background: var(--panel-3);
    color: var(--text-2);
    white-space: nowrap;
  }
  .chip.warn {
    color: var(--warn);
    background: rgba(255, 194, 75, 0.12);
  }
  .spacer {
    flex: 1;
  }
  main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .editor {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  aside {
    width: 360px;
    flex-shrink: 0;
    background: var(--panel);
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .tabs {
    display: flex;
    border-bottom: 1px solid var(--line);
  }
  .tabs button {
    flex: 1;
    justify-content: center;
    border: none;
    border-radius: 0;
    background: transparent;
    padding: 11px;
    color: var(--text-2);
    border-bottom: 2px solid transparent;
  }
  .tabs button.on {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .badge {
    font-size: 11px;
    background: var(--accent);
    color: #fff;
    border-radius: 10px;
    padding: 0 6px;
  }
  .side-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 14px;
    display: flex;
    flex-direction: column;
  }
  .side-foot {
    padding: 12px 14px;
    border-top: 1px solid var(--line);
  }
  .side-foot button {
    width: 100%;
    justify-content: center;
    padding: 9px;
  }
  .welcome {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .drop {
    width: min(560px, 100%);
    padding: 48px 32px;
    border: 2px dashed var(--line-2);
    border-radius: 16px;
    text-align: center;
    display: grid;
    gap: 12px;
    justify-items: center;
    transition: border-color 0.15s, background 0.15s;
  }
  .drop.over {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .drop h2 {
    margin: 0;
    font-size: 20px;
  }
  .drop p {
    margin: 0;
    max-width: 420px;
  }
  .big {
    color: var(--accent);
  }
  .drop .primary {
    padding: 10px 18px;
    font-size: 14px;
  }
  .keys {
    font-size: 12px;
  }
  .dragmask {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(255, 122, 46, 0.12);
    border: 3px dashed var(--accent);
    font-size: 18px;
    pointer-events: none;
    z-index: 40;
  }
  .error {
    position: fixed;
    left: 50%;
    bottom: 20px;
    transform: translateX(-50%);
    max-width: min(720px, 90vw);
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 10px 12px;
    background: #2a1418;
    border: 1px solid var(--err);
    border-radius: var(--radius);
    color: #ffc9cf;
    white-space: pre-wrap;
    user-select: text;
    z-index: 60;
    font-size: 12px;
  }
  .toast {
    position: fixed;
    left: 50%;
    top: 60px;
    transform: translateX(-50%);
    padding: 8px 14px;
    background: var(--panel-3);
    border: 1px solid var(--line-2);
    border-radius: 20px;
    z-index: 60;
    font-size: 12px;
  }
</style>
