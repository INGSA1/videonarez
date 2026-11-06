<script lang="ts">
  import {
    app, captureToggle, captureSave, captureRecord, applyCapture, refreshSources, openInEditor, reveal, setAutostart,
  } from "../lib/store.svelte";
  import { fmtTime, fmtDur } from "../lib/format";
  import Icon from "./Icon.svelte";
  import type { CaptureSettings, Vendor } from "../lib/types";

  const st = $derived(app.cap.status);
  const src = $derived(app.cap.sources);
  const d = $derived(app.cap.draft);
  const encoders = $derived(app.settings?.encoders ?? []);

  const VENDORS: Record<Vendor, string> = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel QuickSync", cpu: "CPU" };
  const vendorsFor = $derived(
    d ? Array.from(new Set(encoders.filter((e) => e.codec === d.codec).map((e) => e.vendor))) : [],
  );

  function touch() {
    app.cap.dirty = true;
  }

  function set<K extends keyof CaptureSettings>(k: K, v: CaptureSettings[K]) {
    if (!d) return;
    d[k] = v;
    touch();
  }

  // Оценка памяти буфера: битрейт × длительность (+ звук).
  const ramMb = $derived(d ? ((d.bitrateMbps * 1000 + 320) * d.bufferSecs) / 8 / 1024 : 0);

  const selectedWindow = $derived(
    d && src ? src.windows.find((w) => w.exe.toLowerCase() === d.windowExe.toLowerCase()) : undefined,
  );

  function pickWindow(hwnd: string) {
    const w = src?.windows.find((x) => String(x.hwnd) === hwnd);
    if (!w || !d) return;
    d.windowExe = w.exe;
    d.windowTitle = w.title;
    touch();
  }

  // Запись сочетания клавиш в формате Tauri: "Alt+F10", "Ctrl+Shift+S".
  let listening = $state<"" | "hotkeySave" | "hotkeyRecord">("");
  function hotkeyKey(e: KeyboardEvent) {
    if (!listening || !d) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") {
      listening = "";
      return;
    }
    if (e.code === "Backspace" || e.code === "Delete") {
      d[listening] = "";
      listening = "";
      touch();
      return;
    }
    if (["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"].includes(e.code)) return;
    const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean);
    const key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : e.code;
    d[listening] = [...mods, key].join("+");
    listening = "";
    touch();
  }

  const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;
  const recording = $derived(st?.recordingSecs != null);
  const armed = $derived(!!st && (st.running || st.waitingForGame));
  const fill = $derived(st && st.maxSecs ? Math.min(100, (st.bufferedSecs / st.maxSecs) * 100) : 0);
</script>

<svelte:window onkeydown={hotkeyKey} />

<div class="capture">
  <!-- Левая колонка: состояние и действия -->
  <div class="col main">
    <section class="status" class:on={st?.running} class:wait={st?.waitingForGame} class:rec={recording}>
      <div class="head">
        <span class="dot"></span>
        <h2>
          {#if st?.starting}Запуск захвата…{:else if recording}Идёт запись · <span class="mono">{fmtTime(st?.recordingSecs ?? 0, 0)}</span>
          {:else if st?.running}Буфер повтора работает{#if st.game}<span class="game"> · {st.game}</span>{/if}
          {:else if st?.waitingForGame}Ожидание полноэкранной игры…{:else}Захват выключен{/if}
        </h2>
      </div>
      {#if st?.waitingForGame}
        <p class="muted">Запустите игру — буфер включится автоматически через пару секунд после того, как она развернётся на весь экран.</p>
      {/if}

      {#if st?.running}
        <div class="buffer">
          <div class="bar"><span style="width:{fill}%"></span></div>
          <div class="nums">
            <span><b class="mono">{Math.floor(st.bufferedSecs)}</b> / {st.maxSecs} с в памяти</span>
            <span class="mono">{st.bufferMb.toFixed(0)} МБ</span>
          </div>
        </div>
        <div class="metrics">
          <div><span class="dim">Кадров/с</span><b class="mono">{st.fps.toFixed(0)}</b></div>
          <div><span class="dim">Битрейт</span><b class="mono">{(st.bitrateKbps / 1000).toFixed(1)} Мбит/с</b></div>
          <div><span class="dim">Кодировщик</span><b class="mono">{st.encoder}</b></div>
          <div><span class="dim">Работает</span><b class="mono">{fmtTime(st.uptimeSecs, 0)}</b></div>
        </div>
        <div class="pipe">
          {#if st.zeroCopy}<span class="badge ok">zero-copy</span>{:else}<span class="badge">копия кадра в ОЗУ</span>{/if}
          <span class="muted">{st.pipeline}</span>
          <span class="dim">· {st.source}</span>
        </div>
      {:else if !st?.waitingForGame}
        <p class="muted">
          Включите буфер — последние {d?.bufferSecs ?? 60} с экрана и звука будут храниться в оперативной памяти.
          По <kbd>{d?.hotkeySave || "—"}</kbd> они мгновенно сохраняются в клип, на диск до этого ничего не пишется.
        </p>
      {/if}

      <div class="actions">
        <button class:primary={!armed} onclick={captureToggle} disabled={app.cap.busy || st?.starting}>
          <Icon name={armed ? "x" : "play"} />
          {armed ? "Выключить буфер" : d?.source === "auto" ? "Включить (ждать игру)" : "Включить буфер повтора"}
        </button>
        <button class="save" onclick={captureSave} disabled={!st?.running}>
          <Icon name="scissors" /> Сохранить повтор {#if d?.hotkeySave}<kbd>{d.hotkeySave}</kbd>{/if}
        </button>
        <button class="recbtn" class:active={recording} onclick={captureRecord} disabled={app.cap.busy}>
          <span class="recdot"></span> {recording ? "Остановить запись" : "Запись"} {#if d?.hotkeyRecord}<kbd>{d.hotkeyRecord}</kbd>{/if}
        </button>
      </div>

      {#if st?.error}<p class="err">{st.error}</p>{/if}
      {#each st?.audioErrors ?? [] as e (e)}<p class="warn">Звук — {e}</p>{/each}
    </section>

    <section class="recent">
      <h4>Сохранённые клипы</h4>
      {#if !app.cap.recent.length}
        <p class="dim">Клипы этой сессии появятся здесь. Они сохраняются в папку {app.settings?.outputDir}.</p>
      {:else}
        {#each app.cap.recent as c (c.path)}
          <div class="clip">
            <Icon name={c.kind === "replay" ? "loop" : "play"} />
            <div class="cinfo">
              <span class="cname" title={c.path}>{fileName(c.path)}</span>
              <span class="dim">{c.kind === "replay" ? "повтор" : "запись"} · {fmtDur(c.duration)}</span>
            </div>
            <button onclick={() => openInEditor(c.path)}><Icon name="scissors" size={14} /> Нарезать</button>
            <button class="ghost icon" title="Показать в проводнике" onclick={() => reveal(c.path)}><Icon name="folder" size={14} /></button>
          </div>
        {/each}
      {/if}
    </section>

    <p class="hint dim">
      Закройте окно — захват продолжит работать из трея, а интерфейс (WebView2) полностью выгрузится из памяти.
      Вернуться — клик по значку Videonarez в трее.
    </p>
  </div>

  <!-- Правая колонка: настройки -->
  <div class="col settings">
    {#if d}
      <section>
        <div class="row-head">
          <h4>Источник</h4>
          <button class="ghost icon" title="Обновить списки" onclick={refreshSources}><Icon name="loop" size={14} /></button>
        </div>
        <div class="seg">
          <button class:active={d.source === "auto"} onclick={() => set("source", "auto")}>Авто: игра</button>
          <button class:active={d.source === "monitor"} onclick={() => set("source", "monitor")}>Монитор</button>
          <button class:active={d.source === "window"} onclick={() => set("source", "window")}>Окно</button>
        </div>
        {#if d.source === "auto"}
          <p class="hint dim">
            Буфер включается сам, когда на переднем плане появляется полноэкранная игра (обычный или borderless
            fullscreen), и выключается, когда игра закрыта. Браузеры, плееры и оболочка Windows не считаются играми.
          </p>
          <label>
            <span>Не считать играми <span class="dim">(exe через запятую)</span></span>
            <input
              placeholder="например: photoshop.exe, premiere.exe"
              value={d.gameExclude.join(", ")}
              onchange={(e) => set("gameExclude", e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean))}
            />
          </label>
        {:else if d.source === "monitor"}
          <select value={d.monitor} onchange={(e) => set("monitor", Number(e.currentTarget.value))}>
            {#each src?.monitors ?? [] as m (m.index)}
              <option value={m.index}>Монитор {m.index + 1} — {m.width}×{m.height}{m.primary ? " (основной)" : ""}</option>
            {/each}
          </select>
          <p class="hint dim">Лучший вариант для полноэкранных игр.</p>
        {:else}
          <select value={selectedWindow ? String(selectedWindow.hwnd) : ""} onchange={(e) => pickWindow(e.currentTarget.value)}>
            <option value="" disabled>{d.windowExe ? `${d.windowExe} (не запущено)` : "Выберите окно…"}</option>
            {#each src?.windows ?? [] as w (w.hwnd)}
              <option value={String(w.hwnd)}>{w.title} — {w.exe}</option>
            {/each}
          </select>
          <p class="hint dim">Окно запоминается по exe-файлу — после перезапуска игры выберется само.</p>
        {/if}
      </section>

      <section>
        <h4>Звук</h4>
        <label class="check"><input type="checkbox" checked={d.systemAudio} onchange={(e) => set("systemAudio", e.currentTarget.checked)} /> Звук игры / системы</label>
        {#if d.source !== "monitor" && d.systemAudio}
          <label class="check sub"><input type="checkbox" checked={d.appAudioOnly} onchange={(e) => set("appAudioOnly", e.currentTarget.checked)} /> Только звук этого окна <span class="dim">(без Discord и музыки)</span></label>
        {/if}
        <label class="check"><input type="checkbox" checked={d.mic} onchange={(e) => set("mic", e.currentTarget.checked)} /> Микрофон <span class="dim">— отдельной дорожкой</span></label>
        {#if d.mic}
          <select value={d.micDevice} onchange={(e) => set("micDevice", e.currentTarget.value)}>
            <option value="">По умолчанию</option>
            {#each src?.mics ?? [] as m (m.id)}<option value={m.id}>{m.name}{m.default ? " ★" : ""}</option>{/each}
          </select>
        {/if}
        <label class="row">
          <span>Сдвиг звука, мс <span class="dim">(если звук опережает видео — плюс)</span></span>
          <input type="number" step="10" min="-1000" max="1000" value={d.audioOffsetMs} onchange={(e) => set("audioOffsetMs", Number(e.currentTarget.value))} />
        </label>
      </section>

      <section class="grid">
        <label>
          <span>Кодек</span>
          <div class="seg">
            <button class:active={d.codec === "h264"} onclick={() => set("codec", "h264")}>H.264</button>
            <button class:active={d.codec === "hevc"} onclick={() => set("codec", "hevc")}>HEVC</button>
          </div>
        </label>
        <label>
          <span>Кодировщик</span>
          <select value={d.encoder} onchange={(e) => set("encoder", e.currentTarget.value as CaptureSettings["encoder"])}>
            <option value="auto">Авто</option>
            {#each vendorsFor as v (v)}<option value={v}>{VENDORS[v]}</option>{/each}
          </select>
        </label>
        <label>
          <span>Разрешение</span>
          <select value={d.height} onchange={(e) => set("height", Number(e.currentTarget.value))}>
            <option value={0}>Исходное</option>
            {#each [1440, 1080, 720] as h (h)}<option value={h}>{h}p</option>{/each}
          </select>
        </label>
        <label>
          <span>FPS</span>
          <select value={d.fps} onchange={(e) => set("fps", Number(e.currentTarget.value))}>
            {#each [30, 60, 120] as f (f)}<option value={f}>{f}</option>{/each}
          </select>
        </label>
      </section>

      <section>
        <h4>Битрейт и буфер</h4>
        <div class="slider">
          <input type="range" min="5" max="80" step="1" value={d.bitrateMbps} oninput={(e) => set("bitrateMbps", Number(e.currentTarget.value))} />
          <span class="mono val">{d.bitrateMbps} Мбит/с</span>
        </div>
        <div class="chips">
          {#each [15, 30, 60, 120, 180, 300] as s (s)}
            <button class="chip" class:active={d.bufferSecs === s} onclick={() => set("bufferSecs", s)}>{s < 60 ? `${s} с` : `${s / 60} мин`}</button>
          {/each}
        </div>
        <p class="hint dim">Буфер займёт ≈ <b>{ramMb.toFixed(0)} МБ</b> оперативной памяти.</p>
      </section>

      <section>
        <h4>Горячие клавиши <span class="dim">(работают и в игре)</span></h4>
        <label class="row">
          <span>Сохранить повтор</span>
          <button class="hk" class:active={listening === "hotkeySave"} onclick={() => (listening = "hotkeySave")}>
            {listening === "hotkeySave" ? "Нажмите сочетание…" : d.hotkeySave || "не задано"}
          </button>
        </label>
        <label class="row">
          <span>Начать / остановить запись</span>
          <button class="hk" class:active={listening === "hotkeyRecord"} onclick={() => (listening = "hotkeyRecord")}>
            {listening === "hotkeyRecord" ? "Нажмите сочетание…" : d.hotkeyRecord || "не задано"}
          </button>
        </label>
        {#each app.cap.hotkeyErrors as e (e)}<p class="warn">Не удалось назначить {e}</p>{/each}
      </section>

      <section>
        <h4>Поведение</h4>
        <label class="check"><input type="checkbox" checked={app.autostart} onchange={(e) => setAutostart(e.currentTarget.checked)} /> Запускать вместе с Windows <span class="dim">(сразу в трей, без окна)</span></label>
        <label class="check"><input type="checkbox" checked={d.autoStart} onchange={(e) => set("autoStart", e.currentTarget.checked)} /> Включать буфер при запуске приложения</label>
        <label class="check"><input type="checkbox" checked={d.closeToTray} onchange={(e) => set("closeToTray", e.currentTarget.checked)} /> Закрытие окна сворачивает в трей</label>
        <label class="check"><input type="checkbox" checked={d.sound} onchange={(e) => set("sound", e.currentTarget.checked)} /> Звуковой сигнал при сохранении клипа</label>
        {#if src?.gpu?.vendor === "amf"}
          <label class="check"><input type="checkbox" checked={d.amfDirect} onchange={(e) => set("amfDirect", e.currentTarget.checked)} /> Прямой захват AMD (zero-copy) <span class="dim">— экспериментально</span></label>
          <p class="hint dim">Меньше нагрузка на CPU (~0,5%), но с этим режимом во время игры был замечен сбой драйвера AMD. По умолчанию — стабильный Windows.Graphics.Capture, как в Xbox Game Bar и OBS.</p>
        {/if}
      </section>

      <div class="apply">
        <button class="primary" disabled={!app.cap.dirty || app.cap.applying} onclick={applyCapture}>
          {app.cap.applying ? "Применяю…" : st?.running ? "Применить и перезапустить захват" : "Применить"}
        </button>
      </div>
    {/if}
  </div>
</div>

<style>
  .capture {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 400px;
  }
  .col {
    overflow: auto;
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .settings {
    background: var(--panel);
    border-left: 1px solid var(--line);
    gap: 18px;
  }
  section {
    display: grid;
    gap: 8px;
  }
  h4 {
    margin: 0;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-3);
    font-weight: 600;
  }
  .status {
    padding: 18px;
    border-radius: 12px;
    background: var(--panel);
    border: 1px solid var(--line);
    gap: 14px;
  }
  .status.on {
    border-color: rgba(61, 220, 151, 0.35);
  }
  .status.rec {
    border-color: rgba(255, 61, 110, 0.5);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: 18px;
  }
  .dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--text-3);
  }
  .on .dot {
    background: var(--ok);
    box-shadow: 0 0 0 4px rgba(61, 220, 151, 0.15);
  }
  .status.wait {
    border-color: rgba(255, 194, 75, 0.4);
  }
  .wait .dot {
    background: var(--warn);
    box-shadow: 0 0 0 4px rgba(255, 194, 75, 0.15);
    animation: pulse 1.6s infinite;
  }
  .game {
    color: var(--text-2);
    font-weight: 400;
  }
  .rec .dot {
    background: var(--accent-2);
    box-shadow: 0 0 0 4px rgba(255, 61, 110, 0.2);
    animation: pulse 1.2s infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.4;
    }
  }
  .buffer .bar {
    height: 8px;
    border-radius: 8px;
    background: var(--panel-3);
    overflow: hidden;
  }
  .buffer .bar span {
    display: block;
    height: 100%;
    background: linear-gradient(90deg, #2bb67a, var(--ok));
    transition: width 0.8s linear;
  }
  .nums {
    display: flex;
    justify-content: space-between;
    margin-top: 6px;
    font-size: 12px;
    color: var(--text-2);
  }
  .metrics {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 8px;
  }
  .metrics div {
    display: grid;
    gap: 2px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--panel-2);
    font-size: 11px;
  }
  .metrics b {
    font-size: 14px;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pipe {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 12px;
    flex-wrap: wrap;
  }
  .badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 10px;
    background: var(--panel-3);
    color: var(--text-2);
  }
  .badge.ok {
    background: rgba(61, 220, 151, 0.14);
    color: var(--ok);
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .actions button {
    padding: 9px 14px;
  }
  .save {
    border-color: var(--accent);
    color: #ffb183;
  }
  .recbtn .recdot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--accent-2);
  }
  .recbtn.active {
    background: rgba(255, 61, 110, 0.15);
    border-color: var(--accent-2);
    color: #ff9db5;
  }
  .err,
  .warn {
    margin: 0;
    font-size: 12px;
    white-space: pre-wrap;
    user-select: text;
  }
  .err {
    color: var(--err);
  }
  .warn {
    color: var(--warn);
  }
  .recent {
    gap: 6px;
  }
  .recent p {
    margin: 0;
  }
  .clip {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--panel);
    border: 1px solid var(--line);
  }
  .cinfo {
    flex: 1;
    min-width: 0;
    display: grid;
    font-size: 12px;
  }
  .cname {
    font-weight: 500;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    margin: 0;
    font-size: 12px;
  }
  .row-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .seg {
    display: flex;
  }
  .seg button {
    flex: 1;
    justify-content: center;
    border-radius: 0;
  }
  .seg button:first-child {
    border-radius: 6px 0 0 6px;
  }
  .seg button:last-child {
    border-radius: 0 6px 6px 0;
  }
  .seg button + button {
    border-left: none;
  }
  .grid {
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  label {
    display: grid;
    gap: 4px;
    font-size: 12px;
    color: var(--text-2);
  }
  label.row {
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: 10px;
  }
  label.row input {
    width: 80px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text);
    font-size: 13px;
  }
  .check.sub {
    margin-left: 22px;
  }
  .slider {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .slider input {
    flex: 1;
  }
  .val {
    min-width: 90px;
    text-align: right;
    color: var(--text);
  }
  .chips {
    display: flex;
    gap: 5px;
    flex-wrap: wrap;
  }
  .chip {
    padding: 3px 10px;
    border-radius: 20px;
    font-size: 12px;
  }
  .hk {
    min-width: 150px;
    justify-content: center;
    font-family: var(--mono);
  }
  .apply {
    position: sticky;
    bottom: 0;
    padding-top: 10px;
    background: linear-gradient(to bottom, transparent, var(--panel) 12px);
  }
  .apply button {
    width: 100%;
    justify-content: center;
    padding: 10px;
  }
</style>
