<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { app, saveSettings, toast } from "../lib/store.svelte";
  import { fmtBytes } from "../lib/format";
  import Icon from "./Icon.svelte";
  import type { EncoderInfo, Priority } from "../lib/types";

  const s = $derived(app.settings);
  let detecting = $state(false);
  let cacheSize = $state<number | null>(null);

  $effect(() => {
    invoke<number>("preview_cache_size").then((n) => (cacheSize = n)).catch(() => {});
  });

  const PRIORITIES: { id: Priority; label: string; hint: string }[] = [
    { id: "idle", label: "Минимальный", hint: "Экспорт идёт только когда CPU простаивает — не мешает играм вообще" },
    { id: "belowNormal", label: "Ниже среднего", hint: "Рекомендуется: игры и интерфейс всегда в приоритете" },
    { id: "normal", label: "Обычный", hint: "Максимальная скорость экспорта" },
  ];

  const VENDOR: Record<string, string> = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel QuickSync", cpu: "CPU" };
  const grouped = $derived.by(() => {
    const g: Record<string, EncoderInfo[]> = {};
    for (const e of s?.encoders ?? []) (g[e.vendor] ??= []).push(e);
    return g;
  });

  async function redetect() {
    detecting = true;
    try {
      const list = await invoke<EncoderInfo[]>("detect_encoders", { force: true });
      if (app.settings) app.settings.encoders = list;
      toast("Кодировщики определены заново");
    } catch (e) {
      toast(String(e));
    } finally {
      detecting = false;
    }
  }

  async function pickDir() {
    const dir = await open({ directory: true, defaultPath: s?.outputDir });
    if (typeof dir === "string" && app.settings) {
      app.settings.outputDir = dir;
      void saveSettings();
    }
  }

  async function clearCache() {
    await invoke("clear_preview_cache");
    cacheSize = 0;
    toast("Кэш предпросмотра очищен");
  }

  function close() {
    app.showSettings = false;
    void saveSettings();
  }
</script>

<div class="backdrop" onclick={close} role="presentation">
  <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" tabindex="-1">
    <header>
      <h3>Настройки</h3>
      <button class="ghost icon" onclick={close}><Icon name="x" /></button>
    </header>

    {#if s}
      <section>
        <h4>Нагрузка на систему</h4>
        <div class="opts">
          {#each PRIORITIES as p (p.id)}
            <button class="opt" class:active={s.priority === p.id} onclick={() => (s.priority = p.id)}>
              <b>{p.label}</b>
              <span class="dim">{p.hint}</span>
            </button>
          {/each}
        </div>
        <label class="row">
          <span>Лимит потоков CPU-кодировщика <span class="dim">(0 = авто)</span></span>
          <input type="number" min="0" max="64" bind:value={s.cpuThreads} />
        </label>
        <p class="dim small">GPU-кодировщики почти не нагружают процессор; лимит потоков влияет только на x264/x265 и точную обрезку.</p>
      </section>

      <section>
        <h4>Папка для клипов</h4>
        <button class="dir" onclick={pickDir}><Icon name="folder" /> <span class="path">{s.outputDir}</span></button>
      </section>

      <section>
        <div class="head">
          <h4>Кодировщики на этом ПК</h4>
          <button onclick={redetect} disabled={detecting}>{detecting ? "Проверка…" : "Определить заново"}</button>
        </div>
        {#if !s.encoders.length}
          <p class="dim">Определяются… (первый запуск — несколько секунд)</p>
        {:else}
          <div class="enc">
            {#each Object.entries(grouped) as [v, list] (v)}
              <div class="enc-row">
                <span class="vendor" class:hw={v !== "cpu"}>{VENDOR[v] ?? v}</span>
                <span class="codecs">
                  {#each list as e (e.name)}<span class="tag mono">{e.name}</span>{/each}
                </span>
              </div>
            {/each}
          </div>
        {/if}
      </section>

      <section>
        <h4>Кэш предпросмотра</h4>
        <div class="row">
          <span class="muted">Прокси-копии для форматов, которые WebView2 не играет напрямую (удаляются через 3 дня). Занято: {cacheSize === null ? "…" : fmtBytes(cacheSize)}</span>
          <button onclick={clearCache} disabled={!cacheSize}>Очистить</button>
        </div>
      </section>

      <section>
        <h4>Горячие клавиши</h4>
        <div class="keys">
          <span><kbd>Пробел</kbd> пуск / пауза</span>
          <span><kbd>←</kbd> <kbd>→</kbd> кадр</span>
          <span><kbd>Shift</kbd>+<kbd>←</kbd> <kbd>→</kbd> 1 секунда</span>
          <span><kbd>J</kbd> <kbd>L</kbd> ±5 секунд, <kbd>K</kbd> пауза</span>
          <span><kbd>I</kbd> <kbd>O</kbd> начало / конец</span>
          <span><kbd>Enter</kbd> добавить клип</span>
          <span><kbd>P</kbd> проиграть выделенное</span>
          <span><kbd>Del</kbd> удалить выбранный клип</span>
          <span><kbd>Esc</kbd> снять выбор клипа</span>
          <span><kbd>+</kbd> <kbd>−</kbd> <kbd>0</kbd> масштаб таймлайна</span>
          <span><kbd>Ctrl</kbd>+<kbd>O</kbd> открыть файл</span>
          <span><kbd>Ctrl</kbd>+<kbd>E</kbd> экспорт</span>
        </div>
      </section>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: grid;
    place-items: center;
    z-index: 50;
  }
  .modal {
    width: min(640px, calc(100vw - 32px));
    max-height: calc(100vh - 48px);
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--line-2);
    border-radius: 12px;
    padding: 18px 20px 20px;
    display: grid;
    gap: 18px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  h3 {
    margin: 0;
    font-size: 16px;
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
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .opts {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }
  .opt {
    flex-direction: column;
    align-items: flex-start;
    white-space: normal;
    text-align: left;
    gap: 3px;
    padding: 8px 10px;
  }
  .opt span {
    font-size: 11px;
  }
  .row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }
  .row input {
    width: 70px;
  }
  .small {
    margin: 0;
    font-size: 12px;
  }
  .dir {
    justify-content: flex-start;
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .enc {
    display: grid;
    gap: 6px;
  }
  .enc-row {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .vendor {
    width: 130px;
    color: var(--text-2);
  }
  .vendor.hw {
    color: var(--ok);
  }
  .codecs {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .tag {
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--panel-3);
  }
  .keys {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px 16px;
    font-size: 12px;
    color: var(--text-2);
  }
</style>
