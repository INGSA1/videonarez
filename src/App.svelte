<script lang="ts">
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import {
    app, init, openFileDialog, openPath, togglePlay, step, skip, setIn, setOut, addSegment,
    removeSegment, playRange, startExport, scheduleSave, player, VIDEO_EXT,
  } from "./lib/store.svelte";
  import { fmtBytes, codecLabel, fmtTime } from "./lib/format";
  import VideoPlayer from "./components/VideoPlayer.svelte";
  import Timeline from "./components/Timeline.svelte";
  import SegmentList from "./components/SegmentList.svelte";
  import ExportPanel from "./components/ExportPanel.svelte";
  import SettingsModal from "./components/SettingsModal.svelte";
  import CapturePanel from "./components/CapturePanel.svelte";
  import QueuePanel from "./components/QueuePanel.svelte";
  import Icon from "./components/Icon.svelte";
  import Timecode from "./components/Timecode.svelte";

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
  const capState = $derived(
    cst?.recordingSecs != null ? "rec" : cst?.running ? "live" : cst?.waitingForGame ? "wait" : "off",
  );

  // Сведения о файле обычной фразой, а не строкой «чипсов».
  const fileFacts = $derived.by(() => {
    const m = app.meta;
    if (!m) return "";
    const parts: string[] = [];
    if (m.hasVideo) parts.push(`${m.width}×${m.height}`, `${+m.fps.toFixed(2)} кадров/с`, codecLabel(m.videoCodec));
    if (m.audioTracks.length) parts.push(m.audioTracks.length === 1 ? "1 дорожка звука" : `${m.audioTracks.length} дорожки звука`);
    parts.push(fmtBytes(m.sizeBytes));
    return parts.join(", ");
  });

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
    <span class="wordmark">videonarez</span>

    <nav class="modes" aria-label="Режим">
      <button class:on={app.mode === "editor"} onclick={() => (app.mode = "editor")}>Редактор</button>
      <button class:on={app.mode === "capture"} onclick={() => (app.mode = "capture")}>
        <span class="tally {capState}" aria-hidden="true"></span> Захват
        {#if cst?.recordingSecs != null}<Timecode value={fmtTime(cst.recordingSecs, 0)} size={11} tone="tally" />
        {:else if cst?.running}<Timecode value={fmtTime(Math.floor(cst.bufferedSecs), 0)} size={11} tone="dim" />{/if}
      </button>
    </nav>

    {#if app.mode === "editor"}
      {#if app.meta}
        <div class="file">
          <span class="name" title={app.meta.path}>{app.meta.fileName}</span>
          <span class="facts">{fileFacts}{#if app.usingProxy}<span class="proxy" title="Формат не воспроизводится напрямую. Экспорт всё равно идёт из оригинала."> Предпросмотр через облегчённую копию</span>{/if}</span>
        </div>
      {/if}
      <div class="spacer"></div>
      <button onclick={openFileDialog} title="Ctrl+O"><Icon name="open" /> Открыть видео</button>
    {:else}
      <div class="spacer"></div>
    {/if}
    <button class="ghost icon" title="Настройки" onclick={() => (app.showSettings = true)}><Icon name="gear" size={17} /></button>
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
              <h2>Перетащите видео в окно</h2>
              <p>Подойдёт запись из игры, MP4, MKV, MOV или WebM. Файл не загружается в память целиком, поэтому даже двухчасовой стрим открывается сразу.</p>
              <button class="primary" onclick={openFileDialog} disabled={app.loading}>
                <Icon name="open" /> {app.loading ? "Открываю" : "Открыть видео"}
              </button>
            </div>
            <dl class="keys">
              <div><dt><kbd>I</kbd> <kbd>O</kbd></dt><dd>начало и конец клипа</dd></div>
              <div><dt><kbd>Enter</kbd></dt><dd>добавить клип в список</dd></div>
              <div><dt><kbd>Пробел</kbd></dt><dd>пуск и пауза</dd></div>
              <div><dt><kbd>Ctrl</kbd> <kbd>E</kbd></dt><dd>экспорт</dd></div>
            </dl>
          </div>
        {/if}
      </div>

      <aside>
        <nav class="tabs">
          <button class:on={app.tab === "clips"} onclick={() => (app.tab = "clips")}>
            Клипы {#if app.segments.length}<span class="count">{app.segments.length}</span>{/if}
          </button>
          <button class:on={app.tab === "export"} onclick={() => (app.tab = "export")}>Экспорт</button>
          <button class:on={app.tab === "queue"} onclick={() => (app.tab = "queue")}>
            Очередь {#if activeJobs}<span class="count">{activeJobs}</span>{/if}
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
            <button class="primary" onclick={() => (app.tab = "export")}>Перейти к экспорту</button>
          </div>
        {/if}
      </aside>
    {/if}
  </main>

  {#if app.dragOver && app.meta}
    <div class="dragmask"><div>Отпустите, чтобы открыть</div></div>
  {/if}

  {#if app.error}
    <div class="error" role="alert">
      <span>{app.error}</span>
      <button class="ghost icon" title="Скрыть" onclick={() => (app.error = "")}><Icon name="x" /></button>
    </div>
  {/if}
  {#if app.toast}<div class="toast" role="status">{app.toast}</div>{/if}

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
    gap: 18px;
    padding: 0 12px 0 18px;
    height: 48px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
    flex-shrink: 0;
  }
  .wordmark {
    font-family: var(--tc);
    font-weight: 500;
    font-size: 14px;
    color: var(--text);
  }
  .modes {
    display: flex;
    gap: 2px;
    height: 100%;
  }
  .modes button {
    height: 100%;
    border-radius: 0;
    background: transparent;
    color: var(--text-3);
    padding: 0 12px;
    box-shadow: inset 0 -2px 0 transparent;
  }
  .modes button:hover:not(:disabled) {
    background: transparent;
    color: var(--text);
  }
  .modes button.on {
    color: var(--text);
    box-shadow: inset 0 -2px 0 var(--text);
  }
  .tally {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--line-2);
  }
  .tally.live {
    background: var(--live);
  }
  .tally.wait {
    background: var(--hold);
  }
  .tally.rec {
    background: var(--tally);
  }
  .file {
    display: flex;
    align-items: baseline;
    gap: 12px;
    min-width: 0;
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 340px;
  }
  .facts {
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .proxy {
    color: var(--hold);
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
    width: 348px;
    flex-shrink: 0;
    background: var(--panel);
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .tabs {
    display: flex;
    padding: 0 8px;
    border-bottom: 1px solid var(--line);
  }
  .tabs button {
    background: transparent;
    border-radius: 0;
    padding: 13px 10px 11px;
    color: var(--text-3);
    box-shadow: inset 0 -2px 0 transparent;
  }
  .tabs button:hover:not(:disabled) {
    background: transparent;
    color: var(--text);
  }
  .tabs button.on {
    color: var(--text);
    box-shadow: inset 0 -2px 0 var(--text);
  }
  .count {
    font-size: 11px;
    color: var(--text-2);
    background: var(--panel-3);
    border-radius: 9px;
    padding: 1px 6px;
  }
  .side-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
  }
  .side-foot {
    padding: 12px 16px;
    border-top: 1px solid var(--line);
  }
  .side-foot button {
    width: 100%;
    justify-content: center;
    padding: 10px;
  }

  /* --- Пустой редактор --- */
  .welcome {
    flex: 1;
    display: grid;
    align-content: center;
    justify-items: start;
    gap: 28px;
    padding: 48px clamp(24px, 8vw, 120px);
  }
  .drop {
    width: min(620px, 100%);
    padding: 36px;
    border: 1px dashed var(--line-2);
    border-radius: var(--radius-lg);
    display: grid;
    gap: 14px;
    justify-items: start;
  }
  .drop.over {
    border-color: var(--cue);
    background: var(--cue-soft);
  }
  .drop h2 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
  }
  .drop p {
    margin: 0;
    color: var(--text-2);
    font-size: 14px;
    line-height: 1.55;
    max-width: 56ch;
  }
  .drop .primary {
    margin-top: 6px;
    padding: 10px 16px;
  }
  .keys {
    margin: 0;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 240px));
    gap: 8px 32px;
    color: var(--text-3);
  }
  .keys div {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .keys dt {
    min-width: 64px;
  }
  .keys dd {
    margin: 0;
  }

  .dragmask {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(245, 197, 24, 0.08);
    outline: 2px dashed var(--cue);
    outline-offset: -8px;
    font-size: 17px;
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
    padding: 10px 10px 10px 14px;
    background: #2b1d1c;
    border-left: 3px solid var(--err);
    border-radius: var(--radius);
    color: var(--text);
    white-space: pre-wrap;
    user-select: text;
    z-index: 60;
  }
  .toast {
    position: fixed;
    left: 50%;
    bottom: 20px;
    transform: translateX(-50%);
    padding: 9px 14px;
    background: var(--text);
    color: var(--bg);
    font-weight: 500;
    border-radius: var(--radius);
    z-index: 60;
  }
</style>
