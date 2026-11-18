<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { app, startExport, cancelExport, reveal, applyPreset, exportAction, exportSegments, saveSettings, copyForDiscord, clipsDir, videoCrop } from "../lib/store.svelte";
  import { ASPECTS, PRESETS, HEIGHTS, VERTICAL_HEIGHTS, FPS_OPTIONS } from "../lib/presets";
  import { fmtTime, fmtBytes, fmtDur } from "../lib/format";
  import Icon from "./Icon.svelte";
  import type { Codec, TrimMode, Vendor } from "../lib/types";

  const o = $derived(app.opts);
  const m = $derived(app.meta);
  const encoders = $derived(app.settings?.encoders ?? []);

  const MODES: { id: TrimMode; label: string; hint: string }[] = [
    { id: "copy", label: "Быстро, исходное качество", hint: "Сохраняет исходное качество. Начало клипа может немного сдвинуться." },
    { id: "smart", label: "Точно по кадрам", hint: "Сохраняет начало клипа по выбранному кадру." },
    { id: "encode", label: "С настройками качества", hint: "Можно выбрать разрешение, качество и размер файла." },
  ];

  const VENDORS: Record<Vendor, string> = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel QuickSync", cpu: "Процессор (x264, x265)" };
  const CODECS: { id: Codec; label: string }[] = [
    { id: "h264", label: "H.264" },
    { id: "hevc", label: "HEVC" },
    { id: "av1", label: "AV1" },
  ];
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

  const crop = $derived(videoCrop());
  const mode = $derived(crop ? "encode" : o.mode);
  const profile = $derived(Object.hasOwn(PRESETS, o.preset) ? o.preset : mode);
  const frameWidth = $derived(crop?.frameSize.w ?? m?.displayWidth ?? 0);
  const frameHeight = $derived(crop?.frameSize.h ?? m?.displayHeight ?? 0);
  const vertical = $derived(frameHeight > frameWidth);
  const heights = $derived(vertical ? VERTICAL_HEIGHTS : HEIGHTS);

  function widthAt(height: number) {
    const ratio = (crop && ASPECTS[o.aspect]) || (m ? m.displayWidth / m.displayHeight : 1);
    return Math.max(2, Math.round(height * ratio / 2) * 2);
  }
  const outRes = $derived(!crop && o.height && frameHeight
    ? `${widthAt(o.height)}×${o.height}`
    : `${frameWidth}×${frameHeight}`);

  // Оценка размера
  const estimate = $derived.by(() => {
    if (!m) return "";
    if (mode !== "encode") {
      const br = m.bitRate ?? (m.sizeBytes * 8) / m.duration;
      return `≈ ${fmtBytes((br / 8) * totalDur)} всего`;
    }
    if (o.rate === "size") return `Целевой размер до ${o.targetMb} МБ на файл`;
    if (o.rate === "bitrate") return `≈ ${fmtBytes(((o.bitrateKbps + (o.mute ? 0 : o.audioBitrateKbps)) * 1000 / 8) * totalDur)} всего`;
    return "";
  });

  async function pickDir() {
    const dir = await open({ directory: true, defaultPath: app.settings?.outputDir });
    if (typeof dir === "string" && app.settings) {
      app.settings.outputDir = dir;
      void saveSettings();
    }
  }

  function chooseProfile(key: string) {
    const method = MODES.find((entry) => entry.id === key);
    if (method) {
      o.mode = method.id;
      custom();
    } else {
      applyPreset(key);
    }
  }

  function custom() {
    o.preset = "custom";
  }

  const curJob = $derived(app.queue.find((j) => j.id === app.currentJob));
  const queuePos = $derived(
    curJob ? app.queue.filter((j) => j.id < curJob.id && (j.status === "queued" || j.status === "running")).length : 0,
  );

  const qualityLabel = $derived(o.quality <= 18 ? "почти без потерь" : o.quality <= 22 ? "высокое" : o.quality <= 27 ? "среднее" : "низкое, маленький файл");
  let options: HTMLDivElement;
  $effect(() => {
    if (app.result && options) options.scrollTop = 0;
  });
</script>

<div class="panel">
  <div class="export-options" bind:this={options}>
    {#if app.result}
      <div class="result">
        <div class="ok" role="status"><Icon name="check" /> {app.result.files.length === 1 ? "Клип сохранён" : "Клипы сохранены"}</div>
        {#each app.result.files as f (f)}
          <button class="ghost file" onclick={() => reveal(f)} title="Показать в проводнике">
            <Icon name="folder" size={14} /> <span class="path">{f.split(/[\\/]/).pop()}</span>
          </button>
          <button class="discord" onclick={() => copyForDiscord(f)} title="Скопировать файл, чтобы вставить в Discord через Ctrl+V">
            <Icon name="copy" /> Скопировать для Discord
          </button>
        {/each}
        {#each app.result.notes as n (n)}<p class="note">{n}</p>{/each}
      </div>
    {/if}

    <label>
      <span>Имя файла</span>
      <input bind:value={app.baseName} />
    </label>
    <section>
      <label>
        <span>Как сохранить</span>
        <select value={profile} onchange={(e) => chooseProfile(e.currentTarget.value)}>
          <optgroup label="Способ сохранения">
            {#each MODES as md (md.id)}
              <option value={md.id} disabled={!!crop && md.id !== "encode"}>{md.label}</option>
            {/each}
          </optgroup>
          <optgroup label="Готовые профили">
            {#each Object.entries(PRESETS) as [key, p] (key)}
              <option value={key}>{p.label}</option>
            {/each}
          </optgroup>
        </select>
      </label>
      <p class="hint dim">{crop ? "Кадрирование применится при сохранении." : PRESETS[profile]?.hint ?? MODES.find((x) => x.id === mode)?.hint}</p>
    </section>
    <section>
      <span class="field-name">Папка сохранения</span>
      <button class="dir" onclick={pickDir} title={clipsDir() || "Выбрать папку сохранения"}>
        <Icon name="folder" /> <span class="path">{clipsDir() || "Выбрать папку"}</span>
      </button>
    </section>
    {#if segs.length > 1}
      <label class="check merge"><input type="checkbox" bind:checked={o.merge} /> Объединить отрезки в один клип</label>
    {/if}
    <details>
      <summary class="disclosure">Дополнительные настройки</summary>
      <div class="advanced">
        {#if mode === "encode"}
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
                <option value="auto">{activeEncoder && o.encoder === "auto" ? `Выбрать сам (${VENDORS[activeEncoder.vendor]})` : "Выбрать сам"}</option>
                {#each vendorsFor as v (v)}<option value={v}>{VENDORS[v]}</option>{/each}
              </select>
            </label>
            <label>
              <span>Разрешение</span>
              <select bind:value={o.height} onchange={custom}>
                <option value={null}>Исходное</option>
                {#each heights as h (h)}<option value={h}>{vertical ? `${widthAt(h)}×${h}` : `${h}p`}</option>{/each}
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
            <h3 class="h">Качество / размер</h3>
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

        {/if}

        {#if m && m.audioTracks.length}
          <section>
            <h3 class="h">Звук</h3>
            <label class="check"><input type="checkbox" bind:checked={o.mute} /> Без звука</label>
            {#if !o.mute}
              {#each m.audioTracks as t (t.index)}
                {@const to = o.tracks[t.index]}
                {#if to}
                  <div class="track">
                    <label class="check">
                      <input type="checkbox" bind:checked={to.enabled} />
                      {t.title || `Дорожка ${t.index + 1}`}
                    </label>
                    {#if mode === "encode"}
                      <input type="range" min="0" max="2" step="0.05" bind:value={to.volume} disabled={!to.enabled} />
                      <span class="mono val">{Math.round(to.volume * 100)}%</span>
                    {/if}
                  </div>
                {/if}
              {/each}
              {#if m.audioTracks.length > 1}
                {#if mode === "encode"}
                  <div class="seg">
                    <button class:active={o.mixdown} onclick={() => (o.mixdown = true)}>Свести в одну</button>
                    <button class:active={!o.mixdown} onclick={() => (o.mixdown = false)}>Оставить раздельными</button>
                  </div>
                  <p class="hint dim">
                    {o.mixdown
                      ? "Включённые дорожки смешаются с выставленной громкостью. Так клип звучит одинаково в любом плеере и в Discord."
                      : "Каждая включённая дорожка останется отдельной, с выставленной громкостью и своим названием."}
                  </p>
                {:else}
                  <p class="hint dim">Для изменения громкости или сведения дорожек выберите «С настройками качества».</p>
                {/if}
              {/if}
              {#if mode === "encode"}
                <label class="row">
                  <span>Битрейт звука</span>
                  <select bind:value={o.audioBitrateKbps}>
                    {#each [64, 96, 128, 160, 192, 256, 320] as b (b)}<option value={b}>{b} кбит/с</option>{/each}
                  </select>
                </label>
              {/if}
            {/if}
          </section>
        {/if}

        <label>
          <span>Формат файла</span>
          <select bind:value={o.container}>
            <option value="mp4">MP4</option>
            <option value="mkv">MKV</option>
          </select>
        </label>
      </div>
    </details>
  </div>

  <div class="footer">
    {#if app.exporting && curJob?.status === "running"}
      <div class="progress" role="status">
        <div class="pbar"><span style="width:{app.progress.percent}%"></span></div>
        <div class="pinfo">
          <span>{app.progress.stage}{app.progress.total > 1 ? `, клип ${app.progress.current} из ${app.progress.total}` : ""}</span>
          <span class="mono">{app.progress.percent.toFixed(0)}%</span>
        </div>
        <button onclick={() => cancelExport()}><Icon name="x" /> Отменить</button>
      </div>
    {:else if app.exporting && curJob?.status === "queued"}
      <div class="progress" role="status">
        <div class="pinfo"><span>{queuePos ? `Заданий перед этим: ${queuePos}` : "Ожидает начала"}</span></div>
        <button onclick={() => cancelExport()}><Icon name="x" /> Убрать из очереди</button>
      </div>
    {/if}
    {#if m}
      <div class="summary">
        <div><b>{files === 1 ? "Один клип" : `Файлов: ${files}`}</b> · {fmtDur(totalDur)}{#if mode === "encode"} · {outRes}{/if}</div>
        {#if estimate}<div class="dim">{estimate}</div>{/if}
      </div>
    {/if}
    <button class="primary go" disabled={!m || !app.settings || totalDur <= 0} onclick={startExport} title="Ctrl+E">
      {exportAction()}
    </button>
  </div>
</div>

<style>
  .panel {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .export-options {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 16px;
    display: grid;
    gap: 14px;
    align-content: start;
  }
  section {
    display: grid;
    gap: 8px;
  }
  .hint {
    margin: 0;
    font-size: 12px;
  }
  details {
    border: 1px solid var(--line-2);
    border-radius: var(--radius-lg);
  }
  .advanced {
    padding: 12px;
    border-top: 1px solid var(--line);
    display: grid;
    gap: 14px;
  }
  .chip {
    padding: 5px 9px;
    font-size: 12px;
    font-weight: 400;
  }
  .chip.small {
    padding: 2px 7px;
    font-size: 11px;
  }
  .grid {
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  label, .field-name {
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
    display: grid;
    gap: 10px;
    padding: 12px 16px 16px;
    background: var(--panel);
    border-top: 1px solid var(--line);
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
  .progress {
    display: grid;
    gap: 6px;
  }
  .discord {
    justify-content: center;
    margin-bottom: 4px;
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
    background: var(--text);
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
    background: var(--panel-2);
    border-left: 3px solid var(--live);
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
