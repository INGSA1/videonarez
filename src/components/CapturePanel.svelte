<script lang="ts">
  import { app, captureToggle, captureSave, captureRecord, applyCapture, refreshSources, setAutostart } from "../lib/store.svelte";
  import { fmtTime } from "../lib/format";
  import Icon from "./Icon.svelte";
  import Timecode from "./Timecode.svelte";
  import ClipGallery from "./ClipGallery.svelte";
  import type { CaptureSettings, Vendor } from "../lib/types";

  const st = $derived(app.cap.status);
  const src = $derived(app.cap.sources);
  const d = $derived(app.cap.draft);
  const encoders = $derived(app.settings?.encoders ?? []);

  const VENDORS: Record<Vendor, string> = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel QuickSync", cpu: "Процессор" };
  const vendorsFor = $derived(
    d ? Array.from(new Set(encoders.filter((e) => e.codec === d.codec).map((e) => e.vendor))) : [],
  );

  function set<K extends keyof CaptureSettings>(k: K, v: CaptureSettings[K]) {
    if (!d) return;
    d[k] = v;
    app.cap.dirty = true;
  }

  const ramMb = $derived(d ? ((d.bitrateMbps * 1000 + 320) * d.bufferSecs) / 8 / 1024 : 0);

  const selectedWindow = $derived(
    d && src ? src.windows.find((w) => w.exe.toLowerCase() === d.windowExe.toLowerCase()) : undefined,
  );

  function pickWindow(hwnd: string) {
    const w = src?.windows.find((x) => String(x.hwnd) === hwnd);
    if (!w || !d) return;
    d.windowExe = w.exe;
    d.windowTitle = w.title;
    app.cap.dirty = true;
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
      app.cap.dirty = true;
      return;
    }
    if (["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"].includes(e.code)) return;
    const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean);
    const key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : e.code;
    d[listening] = [...mods, key].join("+");
    listening = "";
    app.cap.dirty = true;
  }

  const recording = $derived(st?.recordingSecs != null);
  const armed = $derived(!!st && (st.running || st.waitingForGame));
  const fill = $derived(st && st.maxSecs ? Math.min(100, (st.bufferedSecs / st.maxSecs) * 100) : 0);
  const phase = $derived(
    st?.starting ? "start" : recording ? "rec" : st?.running ? "live" : st?.waitingForGame ? "wait" : "off",
  );
  const phaseLabel: Record<string, string> = {
    start: "Запускаю захват",
    rec: "Идёт запись",
    live: "Буфер повтора включён",
    wait: "Жду полноэкранную игру",
    off: "Захват выключен",
  };
</script>

<svelte:window onkeydown={hotkeyKey} />

<div class="capture">
  <div class="col main">
    <section class="deck {phase}">
      <div class="tally-row">
        <span class="tally" aria-hidden="true"></span>
        <h2>{phaseLabel[phase]}</h2>
        {#if st?.game}<span class="game">{st.game}</span>{/if}
      </div>

      {#if recording}
        <Timecode value={fmtTime(st?.recordingSecs ?? 0, 0)} size={44} tone="tally" />
      {:else if st?.running}
        <div class="readout">
          <Timecode value={fmtTime(Math.floor(st.bufferedSecs), 0)} size={44} />
          <span class="of">из</span>
          <Timecode value={fmtTime(st.maxSecs, 0)} size={44} tone="dim" />
        </div>
        <div class="bar"><span style="width:{fill}%"></span></div>
      {:else if st?.waitingForGame}
        <p class="lead">Запустите игру. Буфер включится сам, когда она развернётся на весь экран, и выключится, когда вы её закроете.</p>
      {:else}
        <p class="lead">
          Буфер держит последние {d?.bufferSecs ?? 60} секунд экрана и звука в оперативной памяти. Клавиша
          <kbd>{d?.hotkeySave || "сохранения"}</kbd> превращает их в клип. До этого на диск ничего не пишется.
        </p>
      {/if}

      {#if st?.running}
        <dl class="stats">
          <div><dt>Кадров в секунду</dt><dd class="mono">{st.fps.toFixed(0)}</dd></div>
          <div><dt>Поток</dt><dd class="mono">{(st.bitrateKbps / 1000).toFixed(1)} Мбит/с</dd></div>
          <div><dt>Занято памяти</dt><dd class="mono">{st.bufferMb.toFixed(0)} МБ</dd></div>
          <div><dt>Кодировщик</dt><dd>{st.encoder}</dd></div>
          <div><dt>Захват</dt><dd>{st.zeroCopy ? "Прямой, в видеопамяти" : st.pipeline}</dd></div>
          <div><dt>Источник</dt><dd>{st.source}</dd></div>
        </dl>
      {/if}

      <div class="actions">
        {#if st?.running}
          <button class="primary" onclick={captureSave}>
            <Icon name="scissors" /> Сохранить повтор {#if d?.hotkeySave}<kbd class="onyellow">{d.hotkeySave}</kbd>{/if}
          </button>
        {:else if !armed}
          <button class="primary" onclick={captureToggle} disabled={app.cap.busy || st?.starting}>
            <Icon name="play" /> {d?.source === "auto" ? "Ждать игру" : "Включить буфер"}
          </button>
        {/if}
        <button class="rec" class:on={recording} onclick={captureRecord} disabled={app.cap.busy || st?.waitingForGame}>
          <span class="recdot"></span> {recording ? "Остановить запись" : "Записать"} {#if d?.hotkeyRecord}<kbd>{d.hotkeyRecord}</kbd>{/if}
        </button>
        {#if armed}
          <button class="ghost" onclick={captureToggle} disabled={app.cap.busy}>Выключить буфер</button>
        {/if}
      </div>

      {#if st?.error}<p class="err">{st.error}</p>{/if}
      {#each st?.audioErrors ?? [] as e (e)}<p class="warn">Звук не захватывается. {e}</p>{/each}
    </section>

    <ClipGallery />

    <p class="foot">
      Если закрыть окно, захват продолжит работать из трея, а сам интерфейс выгрузится из памяти.
      Чтобы вернуться, щёлкните значок Videonarez в трее.
    </p>
  </div>

  <aside class="col settings">
    {#if d}
      <section>
        <div class="row-head">
          <h3 class="h">Что записывать</h3>
          <button class="ghost icon" title="Обновить список окон и мониторов" onclick={refreshSources}><Icon name="loop" size={14} /></button>
        </div>
        <div class="seg">
          <button class:active={d.source === "auto"} onclick={() => set("source", "auto")}>Игру</button>
          <button class:active={d.source === "monitor"} onclick={() => set("source", "monitor")}>Монитор</button>
          <button class:active={d.source === "window"} onclick={() => set("source", "window")}>Окно</button>
        </div>
        {#if d.source === "auto"}
          <p class="hint">Любую полноэкранную игру на переднем плане. Браузеры, плееры и рабочий стол не считаются.</p>
          <label>
            <span>Не считать играми</span>
            <input
              placeholder="photoshop.exe, premiere.exe"
              value={d.gameExclude.join(", ")}
              onchange={(e) => set("gameExclude", e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean))}
            />
          </label>
        {:else if d.source === "monitor"}
          <select value={d.monitor} onchange={(e) => set("monitor", Number(e.currentTarget.value))}>
            {#each src?.monitors ?? [] as m (m.index)}
              <option value={m.index}>Монитор {m.index + 1}, {m.width}×{m.height}{m.primary ? ", основной" : ""}</option>
            {/each}
          </select>
        {:else}
          <select value={selectedWindow ? String(selectedWindow.hwnd) : ""} onchange={(e) => pickWindow(e.currentTarget.value)}>
            <option value="" disabled>{d.windowExe ? `${d.windowExe} не запущено` : "Выберите окно"}</option>
            {#each src?.windows ?? [] as w (w.hwnd)}
              <option value={String(w.hwnd)}>{w.title} ({w.exe})</option>
            {/each}
          </select>
          <p class="hint">Окно запоминается по программе и найдётся само после перезапуска.</p>
        {/if}
      </section>

      <section>
        <h3 class="h">Звук</h3>
        <label class="check"><input type="checkbox" checked={d.systemAudio} onchange={(e) => set("systemAudio", e.currentTarget.checked)} /> Звук игры</label>
        {#if d.source !== "monitor" && d.systemAudio}
          <label class="check sub"><input type="checkbox" checked={d.appAudioOnly} onchange={(e) => set("appAudioOnly", e.currentTarget.checked)} /> Только из этой игры, без Discord и музыки</label>
        {/if}
        <label class="check"><input type="checkbox" checked={d.mic} onchange={(e) => set("mic", e.currentTarget.checked)} /> Микрофон отдельной дорожкой</label>
        {#if d.mic}
          <select value={d.micDevice} onchange={(e) => set("micDevice", e.currentTarget.value)}>
            <option value="">Микрофон по умолчанию</option>
            {#each src?.mics ?? [] as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
          </select>
        {/if}
        <label class="row">
          <span>Сдвиг звука, мс</span>
          <input type="number" step="10" min="-1000" max="1000" value={d.audioOffsetMs} onchange={(e) => set("audioOffsetMs", Number(e.currentTarget.value))} />
        </label>
      </section>

      <section>
        <h3 class="h">Качество</h3>
        <div class="grid">
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
              <option value="auto">Выбрать сам</option>
              {#each vendorsFor as v (v)}<option value={v}>{VENDORS[v]}</option>{/each}
            </select>
          </label>
          <label>
            <span>Разрешение</span>
            <select value={d.height} onchange={(e) => set("height", Number(e.currentTarget.value))}>
              <option value={0}>Как на экране</option>
              {#each [1440, 1080, 720] as h (h)}<option value={h}>{h}p</option>{/each}
            </select>
          </label>
          <label>
            <span>Кадров в секунду</span>
            <select value={d.fps} onchange={(e) => set("fps", Number(e.currentTarget.value))}>
              {#each [30, 60, 120] as f (f)}<option value={f}>{f}</option>{/each}
            </select>
          </label>
        </div>
        <label>
          <span>Поток, {d.bitrateMbps} Мбит/с</span>
          <input type="range" min="5" max="80" step="1" value={d.bitrateMbps} oninput={(e) => set("bitrateMbps", Number(e.currentTarget.value))} />
        </label>
      </section>

      <section>
        <h3 class="h">Длина повтора</h3>
        <div class="seg">
          {#each [15, 30, 60, 120, 300] as s (s)}
            <button class:active={d.bufferSecs === s} onclick={() => set("bufferSecs", s)}>{s < 60 ? `${s} с` : `${s / 60} мин`}</button>
          {/each}
        </div>
        <p class="hint">Займёт около {ramMb.toFixed(0)} МБ оперативной памяти.</p>
      </section>

      <section>
        <h3 class="h">Горячие клавиши</h3>
        <label class="row">
          <span>Сохранить повтор</span>
          <button class="hk" class:listening={listening === "hotkeySave"} onclick={() => (listening = "hotkeySave")}>
            {listening === "hotkeySave" ? "Нажмите сочетание" : d.hotkeySave || "Не задано"}
          </button>
        </label>
        <label class="row">
          <span>Начать или остановить запись</span>
          <button class="hk" class:listening={listening === "hotkeyRecord"} onclick={() => (listening = "hotkeyRecord")}>
            {listening === "hotkeyRecord" ? "Нажмите сочетание" : d.hotkeyRecord || "Не задано"}
          </button>
        </label>
        <p class="hint">Работают и внутри игры. Backspace стирает сочетание.</p>
        {#each app.cap.hotkeyErrors as e (e)}<p class="warn">Сочетание занято другой программой: {e}</p>{/each}
      </section>

      <section>
        <h3 class="h">Запуск</h3>
        <label class="check"><input type="checkbox" checked={app.autostart} onchange={(e) => setAutostart(e.currentTarget.checked)} /> Запускать вместе с Windows, сразу в трей</label>
        <label class="check"><input type="checkbox" checked={d.autoStart} onchange={(e) => set("autoStart", e.currentTarget.checked)} /> Включать буфер при запуске</label>
        <label class="check"><input type="checkbox" checked={d.closeToTray} onchange={(e) => set("closeToTray", e.currentTarget.checked)} /> Закрывать окно в трей</label>
        <label class="check"><input type="checkbox" checked={d.sound} onchange={(e) => set("sound", e.currentTarget.checked)} /> Сигнал при сохранении клипа</label>
        {#if src?.gpu?.vendor === "amf"}
          <label class="check"><input type="checkbox" checked={d.amfDirect} onchange={(e) => set("amfDirect", e.currentTarget.checked)} /> Прямой захват AMD</label>
          <p class="hint">Экономит около процента процессора, но с ним во время игры был замечен сбой видеодрайвера. Включайте, только если обычный захват не подходит.</p>
        {/if}
      </section>

      <div class="apply">
        <button class="primary" disabled={!app.cap.dirty || app.cap.applying} onclick={applyCapture}>
          {app.cap.applying ? "Применяю" : st?.running ? "Применить и перезапустить" : "Применить"}
        </button>
      </div>
    {/if}
  </aside>
</div>

<style>
  .capture {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 380px;
  }
  .col {
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
  .main {
    padding: 28px 32px;
    gap: 36px;
  }
  .settings {
    background: var(--panel);
    padding: 22px 22px 0;
    gap: 26px;
  }

  /* --- Пульт состояния --- */
  .deck {
    display: grid;
    gap: 18px;
    max-width: 880px;
  }
  .tally-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .tally {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--line-2);
  }
  .live .tally {
    background: var(--live);
  }
  .wait .tally {
    background: var(--hold);
  }
  .rec .tally {
    background: var(--tally);
    animation: blink 1.2s steps(2, jump-none) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0.25;
    }
  }
  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }
  .game {
    color: var(--text-2);
    padding-left: 10px;
    border-left: 1px solid var(--line-2);
  }
  .readout {
    display: flex;
    align-items: baseline;
    gap: 14px;
  }
  .of {
    color: var(--text-3);
    font-size: 15px;
  }
  .bar {
    height: 3px;
    background: var(--line);
    max-width: 560px;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--live);
    transition: width 0.9s linear;
  }
  .lead {
    margin: 0;
    font-size: 15px;
    line-height: 1.55;
    color: var(--text-2);
    max-width: 62ch;
  }
  .stats {
    margin: 0;
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 14px 28px;
    max-width: 760px;
  }
  .stats div {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  dt {
    font-size: 12px;
    color: var(--text-3);
  }
  dd {
    margin: 0;
    font-size: 14px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .actions button {
    padding: 9px 14px;
  }
  kbd.onyellow {
    background: rgba(27, 28, 30, 0.14);
    color: var(--cue-ink);
    box-shadow: none;
  }
  .recdot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--tally);
  }
  .rec.on {
    background: var(--tally);
    color: #fff;
  }
  .rec.on .recdot {
    background: #fff;
  }
  .err,
  .warn {
    margin: 0;
    font-size: 13px;
    white-space: pre-wrap;
    user-select: text;
    max-width: 80ch;
  }
  .err {
    color: var(--err);
  }
  .warn {
    color: var(--hold);
  }
  .foot {
    margin: auto 0 0;
    font-size: 12px;
    color: var(--text-3);
    max-width: 70ch;
  }

  /* --- Настройки --- */
  section {
    display: grid;
    gap: 9px;
  }
  .row-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--text-3);
  }
  .seg {
    display: flex;
    background: var(--bg);
    border-radius: var(--radius);
    padding: 2px;
    gap: 2px;
  }
  .seg button {
    flex: 1;
    justify-content: center;
    background: transparent;
    padding: 6px 4px;
    color: var(--text-2);
  }
  .seg button.active {
    background: var(--panel-3);
    color: var(--text);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  label {
    display: grid;
    gap: 5px;
    font-size: 12px;
    color: var(--text-2);
  }
  label.row {
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: 10px;
    font-size: 13px;
  }
  label.row input {
    width: 80px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text);
    font-size: 13px;
  }
  .check.sub {
    margin-left: 23px;
    color: var(--text-2);
  }
  .hk {
    min-width: 128px;
    justify-content: center;
    background: var(--bg);
  }
  .hk.listening {
    background: var(--cue-soft);
    color: var(--cue-text);
  }
  .apply {
    position: sticky;
    bottom: 0;
    margin-top: auto;
    padding: 14px 0 18px;
    background: var(--panel);
  }
  .apply button {
    width: 100%;
    justify-content: center;
    padding: 10px;
  }
</style>
