<script lang="ts">
  import { app, addSegment, removeSegment, selectSegment, seek, segmentsFromPeaks, playRange } from "../lib/store.svelte";
  import { fmtTime, parseTime, clamp } from "../lib/format";
  import Icon from "./Icon.svelte";
  import type { Segment } from "../lib/types";

  const total = $derived(app.segments.reduce((a, s) => a + (s.end - s.start), 0));
  const peaks = $derived(app.waveform?.markers.length ?? 0);

  function editTime(s: Segment, key: "start" | "end", value: string) {
    const t = parseTime(value);
    const d = app.meta?.duration ?? 0;
    if (t === null) return;
    if (key === "start") s.start = clamp(t, 0, s.end - 0.1);
    else s.end = clamp(t, s.start + 0.1, d);
  }

  function play(s: Segment) {
    app.selectedId = s.id;
    playRange();
  }
</script>

<div class="list">
  <div class="actions">
    <button onclick={() => addSegment()} disabled={!app.meta || app.selectedId !== null} title="Добавить текущее выделение (Enter)">
      <Icon name="plus" /> Добавить выделение
    </button>
    <button onclick={() => segmentsFromPeaks()} disabled={!peaks} title="Создать клипы вокруг громких моментов (8 с до пика, 4 с после)">
      <Icon name="spark" /> Из пиков{peaks ? ` (${peaks})` : ""}
    </button>
  </div>

  {#if !app.segments.length}
    <div class="empty">
      <p>Клипов пока нет.</p>
      <p class="dim">
        Отметьте начало <kbd>I</kbd> и конец <kbd>O</kbd>, затем <kbd>Enter</kbd> — фрагмент попадёт в список.
        Без списка экспортируется текущее выделение.
      </p>
      {#if app.waveLoading}<p class="dim">Ищу громкие моменты в звуке…</p>{/if}
    </div>
  {:else}
    <div class="summary dim">
      {app.segments.length} шт. · всего <span class="mono">{fmtTime(total, 1)}</span>
      {#if app.selectedId !== null}<span class="editing">· ручки таймлайна редактируют выбранный клип</span>{/if}
    </div>
    <div class="rows">
      {#each app.segments as s, i (s.id)}
        <div class="row" class:sel={s.id === app.selectedId}>
          <button class="num ghost" onclick={() => { selectSegment(s.id); seek(s.start); }} title="Выбрать для редактирования">
            {i + 1}
          </button>
          <div class="body">
            <input class="name" placeholder="Название клипа" bind:value={s.name} />
            <div class="times">
              <input class="mono" value={fmtTime(s.start)} onchange={(e) => editTime(s, "start", e.currentTarget.value)} />
              <span class="dim">→</span>
              <input class="mono" value={fmtTime(s.end)} onchange={(e) => editTime(s, "end", e.currentTarget.value)} />
              <span class="dur mono">{fmtTime(s.end - s.start, 1)}</span>
            </div>
          </div>
          <div class="btns">
            <button class="ghost icon" title="Проиграть" onclick={() => play(s)}><Icon name="play" size={14} /></button>
            <button class="ghost icon del" title="Удалить" onclick={() => removeSegment(s.id)}><Icon name="trash" size={14} /></button>
          </div>
        </div>
      {/each}
    </div>
    <button class="ghost clear" onclick={() => { app.segments = []; app.selectedId = null; }}>Очистить список</button>
  {/if}
</div>

<style>
  .list {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-height: 0;
    flex: 1;
  }
  .actions {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .actions button {
    justify-content: center;
  }
  .empty {
    padding: 18px 6px;
    text-align: center;
  }
  .empty p {
    margin: 0 0 8px;
  }
  .summary {
    font-size: 12px;
  }
  .editing {
    color: #ffb183;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 6px;
    overflow: auto;
    min-height: 0;
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 8px;
    border-radius: var(--radius);
    background: var(--panel-2);
    border: 1px solid var(--line);
  }
  .row.sel {
    border-color: var(--accent);
    background: #201a17;
  }
  .num {
    width: 26px;
    height: 26px;
    padding: 0;
    justify-content: center;
    border-radius: 6px;
    background: var(--panel-3);
    font-weight: 600;
    font-size: 12px;
  }
  .row.sel .num {
    background: var(--accent);
    color: #fff;
  }
  .body {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 4px;
  }
  .name {
    background: transparent;
    border-color: transparent;
    padding: 2px 4px;
    font-weight: 500;
  }
  .name:hover {
    border-color: var(--line-2);
  }
  .times {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .times input {
    width: 82px;
    padding: 2px 4px;
    font-size: 12px;
  }
  .dur {
    margin-left: auto;
    font-size: 11px;
    color: #ffb183;
  }
  .btns {
    display: flex;
    flex-direction: column;
  }
  .del:hover {
    color: var(--err);
  }
  .clear {
    align-self: flex-start;
    font-size: 12px;
    color: var(--text-3);
  }
</style>
