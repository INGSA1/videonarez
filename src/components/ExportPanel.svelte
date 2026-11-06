<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { app, startExport, cancelExport, reveal, applyPreset, exportSegments, saveSettings } from "../lib/store.svelte";
  import { PRESETS, HEIGHTS, VERTICAL_HEIGHTS, FPS_OPTIONS, cropRect } from "../lib/presets";
  import { fmtTime, fmtBytes, fmtDur } from "../lib/format";
  import Icon from "./Icon.svelte";
  import type { AspectKey, Codec, TrimMode, Vendor } from "../lib/types";

  const o = $derived(app.opts);
  const m = $derived(app.meta);
  const encoders = $derived(app.settings?.encoders ?? []);

  const MODES: { id: TrimMode; label: string; icon: string; hint: string }[] = [
    { id: "copy", label: "Мгновенно", icon: "bolt", hint: "Без перекодирования: ~0 нагрузки, без потери качества. Режет по ключевым кадрам — начало может сдвинуться на долю секунды." },
    { id: "smart", label: "Точно", icon: "target", hint: "Перекодируется только кусочек до первого ключевого кадра, остальное копируется. Точность до кадра за 1–2 секунды." },
    { id: "encode", label: "Сжать / изменить", icon: "chip", hint: "Полное перекодирование на видеокарте: разрешение, FPS, кадрирование, размер под Discord." },
  ];

  const VENDORS: Record<Vendor, string> = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel QuickSync", cpu: "CPU (x264/x265)" };
  const CODECS: { id: Codec; label: string }[] = [
    { id: "h264", label: "H.264" },
    { id: "hevc", label: "HEVC" },
    { id: "av1", label: "AV1" },
  ];
  const ASPECT_KEYS: AspectKey[] = ["none", "9:16", "1:1", "4:5", "16:9", "4:3"];

  const codecAvailable = (c: Codec) => encoders.length === 0 || encoders.some((e) => e.codec === c);
  const vendorsFor = $derived(Array.from(new Set(encoders.filter((e) => e.codec === o.codec).map((e) => e.vendor))));
  const activeEncoder = $derived.by(() => {
    const list = encoders.filter((e) => e.codec === o.codec);
    if (o.encoder !== "auto") return list.find((e) => e.vendor === o.encoder);
    return ["nvenc", "amf", "qsv", "cpu"].map((v) => list.find((e) => e.vendor === v)).find(Boolean);
  });

  const segs = $derived(exportSegments());
  const totalDur = $derived(segs.reduce((a, s) => a + (s.end - s.start), 0));
  const files = $derived(o.merge && segs.length > 1 ? 1 : segs.length);

  const crop = $derived(m && o.mode === "encode" ? cropRect(m.displayWidth, m.displayHeight, o.aspect, o.cropX, o.cropY) : null);
  const vertical = $derived(crop ? crop.h > crop.w : m ? m.displayHeight > m.displayWidth : false);
  const heights = $derived(vertical ? VERTICAL_HEIGHTS : HEIGHTS);

  const outRes = $derived.by(() => {
    if (!m) return "";
    let w = crop?.w ?? m.displayWidth;
    let h = crop?.h ?? m.displayHeight;
    if (o.mode === "encode" && o.height) {
      w = Math.round((w * o.height) / h / 2) * 2;
      h = o.height;
    }
    return `${w}×${h}`;
  });

  // Оценка размера
  const estimate = $derived.by(() => {
    if (!m) return "";
    const per = o.merge ? totalDur : Math.max(...segs.map((s) => s.end - s.start));
    if (o.mode !== "encode") {
      const br = m.bitRate ?? (m.sizeBytes * 8) / m.duration;
      return `≈ ${fmtBytes((br / 8) * totalDur)} всего`;
    }
    if (o.rate === "size") {
      const kbps = (o.targetMb * 8192) / Math.max(0.1, per) * 0.97 - (o.mute ? 0 : o.audioBitrateKbps);
      return `≤ ${o.targetMb} МБ на файл · видео ~${Math.max(0, Math.round(kbps))} кбит/с`;
    }
    if (o.rate === "bitrate") return `≈ ${fmtBytes(((o.bitrateKbps + (o.mute ? 0 : o.audioBitrateKbps)) * 1000 / 8) * totalDur)} всего`;
    return "размер зависит от динамики сцены";
  });

  async function pickDir() {
    const dir = await open({ directory: true, defaultPath: app.settings?.outputDir });
    if (typeof dir === "string" && app.settings) {
      app.settings.outputDir = dir;
      void saveSettings();
    }
  }

  function setMode(id: TrimMode) {
    o.mode = id;
    o.preset = "custom";
  }

  function custom() {
    o.preset = "custom";
  }

  const curJob = $derived(app.queue.find((j) => j.id === app.currentJob));
  const activeJobs = $derived(app.queue.filter((j) => j.status === "queued" || j.status === "running").length);
  const queueBusy = $derived(activeJobs > 0);
  const queuePos = $derived(
    curJob ? app.queue.filter((j) => j.id < curJob.id && (j.status === "queued" || j.status === "running")).length : 0,
  );

  const qualityLabel = $derived(o.quality <= 18 ? "почти без потерь" : o.quality <= 22 ? "высокое" : o.quality <= 27 ? "среднее" : "низкое, маленький файл");
</script>

<div class="panel">
  <!-- Режим -->
  <section>
    <div class="modes">
      {#each MODES as md (md.id)}
        <button class="mode" class:active={o.mode === md.id} onclick={() => setMode(md.id)} title={md.hint}>
          <Icon name={md.icon} size={18} />
          <span>{md.label}</span>
        </button>
      {/each}
    </div>
    <p class="hint dim">{MODES.find((x) => x.id === o.mode)?.hint}</p>
  </section>

  {#if o.mode === "encode"}
    <section>
      <h4>Пресет</h4>
      <div class="presets">
        {#each Object.entries(PRESETS) as [key, p] (key)}
          <button class="chip" class:active={o.preset === key} title={p.hint} onclick={() => applyPreset(key)}>{p.label}</button>
        {/each}
      </div>
    </section>

    <section class="grid">
      <label>
        <span>Кодек</span>
        <div class="seg">
          {#each CODECS as c (c.id)}
            <button class:active={o.codec === c.id} disabled={!codecAvailable(c.id)} onclick={() => { o.codec = c.id; custom(); }}>{c.label}</button>
          {/each}
        </div>
      </label>
      <label>
        <span>Кодировщик</span>
        <select bind:value={o.encoder} onchange={custom}>
          <option value="auto">Авто{activeEncoder && o.encoder === "auto" ? ` · ${VENDORS[activeEncoder.vendor]}` : ""}</option>
          {#each vendorsFor as v (v)}<option value={v}>{VENDORS[v]}</option>{/each}
        </select>
      </label>
      <label>
        <span>Разрешение</span>
        <select bind:value={o.height} onchange={custom}>
          <option value={null}>Исходное</option>
          {#each heights as h (h)}<option value={h}>{vertical ? `${Math.round((h * 9) / 16)}×${h}` : `${h}p`}</option>{/each}
        </select>
      </label>
      <label>
        <span>FPS</span>
        <select bind:value={o.fps} onchange={custom}>
          <option value={null}>Исходный{m ? ` (${+m.fps.toFixed(2)})` : ""}</option>
          {#each FPS_OPTIONS as f (f)}<option value={f}>{f}</option>{/each}
        </select>
      </label>
    </section>

    <section>
      <h4><Icon name="crop" size={13} /> Кадрирование</h4>
      <div class="presets">
        {#each ASPECT_KEYS as a (a)}
          <button class="chip" class:active={o.aspect === a} onclick={() => { o.aspect = a; o.cropX = 0.5; o.cropY = 0.5; custom(); }}>
            {a === "none" ? "Без обрезки" : a}
          </button>
        {/each}
      </div>
      {#if crop}<p class="hint dim">Перетащите рамку на видео, чтобы выбрать область.</p>{/if}
    </section>

    <section>
      <h4>Качество / размер</h4>
      <div class="seg wide">
        <button class:active={o.rate === "quality"} onclick={() => { o.rate = "quality"; custom(); }}>Качество</button>
        <button class:active={o.rate === "bitrate"} onclick={() => { o.rate = "bitrate"; custom(); }}>Битрейт</button>
        <button class:active={o.rate === "size"} onclick={() => { o.rate = "size"; custom(); }}>Размер файла</button>
      </div>
      {#if o.rate === "quality"}
        <div class="slider">
          <input type="range" min="14" max="36" step="1" bind:value={o.quality} oninput={custom} />
          <span class="mono val">{o.quality}</span>
        </div>
        <p class="hint dim">Чем меньше число, тем лучше: {qualityLabel}.</p>
      {:else if o.rate === "bitrate"}
        <div class="inline">
          <input type="number" min="0.5" max="100" step="0.5" value={o.bitrateKbps / 1000}
            onchange={(e) => { o.bitrateKbps = Math.round(Number(e.currentTarget.value) * 1000); custom(); }} />
          <span class="dim">Мбит/с</span>
        </div>
      {:else}
        <div class="inline">
          <input type="number" min="1" max="4000" step="1" bind:value={o.targetMb} oninput={custom} />
          <span class="dim">МБ на файл</span>
          {#each [8, 10, 25, 50] as mb (mb)}
            <button class="chip small" class:active={o.targetMb === mb} onclick={() => { o.targetMb = mb; custom(); }}>{mb}</button>
          {/each}
        </div>
      {/if}
    </section>

    {#if m && m.audioTracks.length}
      <section>
        <h4>Звук</h4>
        <label class="check"><input type="checkbox" bind:checked={o.mute} /> Без звука</label>
        {#if !o.mute}
          {#each m.audioTracks as t (t.index)}
            {@const to = o.tracks[t.index]}
            {#if to}
            <div class="track">
              <label class="check">
                <input type="checkbox" bind:checked={to.enabled} />
                {t.title || `Дорожка ${t.index + 1}`}
                <span class="dim">{t.codec} · {t.channels}ch</span>
              </label>
              <input type="range" min="0" max="2" step="0.05" bind:value={to.volume} disabled={!to.enabled} />
              <span class="mono val">{Math.round(to.volume * 100)}%</span>
            </div>
            {/if}
          {/each}
          {#if m.audioTracks.length > 1}<p class="hint dim">Включённые дорожки сводятся в одну (например, игра + микрофон).</p>{/if}
          <label class="row">
            <span>Битрейт звука</span>
            <select bind:value={o.audioBitrateKbps}>
              {#each [64, 96, 128, 160, 192, 256, 320] as b (b)}<option value={b}>{b} кбит/с</option>{/each}
            </select>
          </label>
        {/if}
      </section>
    {/if}
  {/if}

  <section class="grid">
    <label>
      <span>Контейнер</span>
      <div class="seg">
        <button class:active={o.container === "mp4"} onclick={() => (o.container = "mp4")}>MP4</button>
        <button class:active={o.container === "mkv"} onclick={() => (o.container = "mkv")}>MKV</button>
      </div>
    </label>
    <label>
      <span>Имя файла</span>
      <input bind:value={app.baseName} />
    </label>
  </section>

  {#if segs.length > 1}
    <label class="check merge"><input type="checkbox" bind:checked={o.merge} /> Склеить {segs.length} клипа(ов) в один хайлайт</label>
  {/if}

  <section>
    <h4>Папка</h4>
    <button class="dir" onclick={pickDir} title={app.settings?.outputDir}>
      <Icon name="folder" /> <span class="path">{app.settings?.outputDir ?? "—"}</span>
    </button>
  </section>

  <div class="footer">
    <div class="summary">
      <div><b>{files}</b> файл(ов) · <span class="mono">{fmtDur(totalDur)}</span>{#if o.mode === "encode"} · {outRes} · {o.codec.toUpperCase()}{/if}</div>
      <div class="dim">{estimate}</div>
    </div>

    {#if app.exporting && curJob?.status === "running"}
      <div class="progress">
        <div class="pbar"><span style="width:{app.progress.percent}%"></span></div>
        <div class="pinfo">
          <span>{app.progress.stage}{app.progress.total > 1 ? ` · ${app.progress.current}/${app.progress.total}` : ""}</span>
          <span class="mono">{app.progress.percent.toFixed(0)}%</span>
        </div>
        <button onclick={() => cancelExport()}><Icon name="x" /> Отменить</button>
      </div>
    {:else if app.exporting && curJob?.status === "queued"}
      <div class="progress">
        <div class="pinfo"><span>В очереди · перед ним заданий: {queuePos}</span></div>
        <button onclick={() => cancelExport()}><Icon name="x" /> Убрать из очереди</button>
      </div>
    {/if}
    <button class="primary go" disabled={!m || totalDur <= 0} onclick={startExport}>
      <Icon name="scissors" /> {queueBusy ? "Добавить в очередь" : "Экспортировать"} <kbd>Ctrl+E</kbd>
    </button>
    {#if queueBusy}
      <button class="ghost qlink" onclick={() => (app.tab = "queue")}>Очередь экспорта: {activeJobs} в работе →</button>
    {/if}

    {#if app.result}
      <div class="result">
        <div class="ok"><Icon name="check" /> Готово за {app.result.elapsedSec.toFixed(1)} с</div>
        {#each app.result.files as f (f)}
          <button class="ghost file" onclick={() => reveal(f)} title="Показать в проводнике">
            <Icon name="folder" size={14} /> <span class="path">{f.split(/[\\/]/).pop()}</span>
          </button>
        {/each}
        {#each app.result.notes as n (n)}<p class="note">{n}</p>{/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
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
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .modes {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }
  .mode {
    flex-direction: column;
    gap: 4px;
    padding: 10px 4px;
    justify-content: center;
    font-size: 12px;
  }
  .hint {
    margin: 0;
    font-size: 12px;
  }
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .chip {
    padding: 3px 9px;
    border-radius: 20px;
    font-size: 12px;
  }
  .chip.small {
    padding: 2px 7px;
    font-size: 11px;
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
  }
  .seg {
    display: flex;
  }
  .seg button {
    flex: 1;
    border-radius: 0;
    justify-content: center;
    padding: 5px 4px;
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
  .slider,
  .inline {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .slider input {
    flex: 1;
  }
  .inline input {
    width: 76px;
  }
  .val {
    min-width: 36px;
    text-align: right;
    color: var(--text);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text);
    font-size: 13px;
  }
  .track {
    display: grid;
    grid-template-columns: 1fr 90px 40px;
    align-items: center;
    gap: 8px;
  }
  .merge {
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--panel-2);
    border: 1px solid var(--line);
  }
  .dir {
    width: 100%;
    justify-content: flex-start;
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .footer {
    position: sticky;
    bottom: 0;
    display: grid;
    gap: 10px;
    padding: 12px 0 2px;
    background: linear-gradient(to bottom, transparent, var(--panel) 14px);
  }
  .summary {
    font-size: 12px;
    display: grid;
    gap: 2px;
  }
  .go {
    width: 100%;
    justify-content: center;
    padding: 10px;
    font-size: 14px;
  }
  .go kbd {
    background: rgba(0, 0, 0, 0.2);
    border-color: rgba(255, 255, 255, 0.25);
    color: #fff;
  }
  .progress {
    display: grid;
    gap: 6px;
  }
  .qlink {
    justify-content: center;
    font-size: 12px;
    color: var(--text-2);
  }
  .pbar {
    height: 6px;
    background: var(--panel-3);
    border-radius: 6px;
    overflow: hidden;
  }
  .pbar span {
    display: block;
    height: 100%;
    background: linear-gradient(90deg, var(--accent), var(--accent-2));
    transition: width 0.25s;
  }
  .pinfo {
    display: flex;
    justify-content: space-between;
    font-size: 12px;
    color: var(--text-2);
  }
  .result {
    display: grid;
    gap: 4px;
    padding: 10px;
    border-radius: var(--radius);
    background: rgba(61, 220, 151, 0.07);
    border: 1px solid rgba(61, 220, 151, 0.3);
  }
  .ok {
    color: var(--ok);
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
  }
  .file {
    justify-content: flex-start;
    font-size: 12px;
    padding: 3px 6px;
  }
  .note {
    margin: 0;
    font-size: 12px;
    color: var(--warn);
  }
</style>
