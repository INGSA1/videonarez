<script lang="ts">
  import { app, cancelExport, clearQueue, reveal, openInEditor } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import type { JobStatus } from "../lib/types";

  const LABEL: Record<JobStatus, string> = {
    queued: "в очереди",
    running: "идёт",
    done: "готово",
    failed: "ошибка",
    cancelled: "отменено",
  };
  const MODE: Record<string, string> = { copy: "мгновенно", smart: "точно", encode: "перекодирование" };
  const finished = $derived(app.queue.some((j) => j.status !== "queued" && j.status !== "running"));
  const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;
</script>

<div class="queue">
  <p class="hint dim">
    Задания выполняются по одному в фоне с пониженным приоритетом. Можно открыть следующий файл,
    разметить клипы и добавить его в очередь, не дожидаясь окончания.
  </p>

  {#if !app.queue.length}
    <div class="empty dim">Очередь пуста.</div>
  {:else}
    {#each [...app.queue].reverse() as j (j.id)}
      <div class="job {j.status}">
        <div class="top">
          <span class="name" title={j.input}>{j.label}</span>
          <span class="st">{LABEL[j.status]}</span>
        </div>
        <div class="dim meta">{MODE[j.mode] ?? j.mode}{j.elapsedSec ? ` · ${j.elapsedSec.toFixed(1)} с` : ""}</div>
        {#if j.status === "running"}
          <div class="pbar"><span style="width:{app.progress.percent}%"></span></div>
          <div class="dim meta">{app.progress.stage} · {app.progress.percent.toFixed(0)}%</div>
        {/if}
        {#if j.error}<p class="err">{j.error}</p>{/if}
        {#each j.files as f (f)}
          <div class="file">
            <button class="ghost" onclick={() => reveal(f)} title="Показать в проводнике"><Icon name="folder" size={13} /> {fileName(f)}</button>
            <button class="ghost icon" title="Открыть в редакторе" onclick={() => openInEditor(f)}><Icon name="scissors" size={13} /></button>
          </div>
        {/each}
        {#each j.notes as n (n)}<p class="note">{n}</p>{/each}
        {#if j.status === "queued" || j.status === "running"}
          <button class="cancel" onclick={() => cancelExport(j.id)}><Icon name="x" size={13} /> {j.status === "running" ? "Отменить" : "Убрать"}</button>
        {/if}
      </div>
    {/each}
    {#if finished}<button class="ghost clear" onclick={clearQueue}>Очистить завершённые</button>{/if}
  {/if}
</div>

<style>
  .queue {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .hint {
    margin: 0;
    font-size: 12px;
  }
  .empty {
    padding: 24px 0;
    text-align: center;
  }
  .job {
    display: grid;
    gap: 4px;
    padding: 10px;
    border-radius: var(--radius);
    background: var(--panel-2);
    border: 1px solid var(--line);
  }
  .job.running {
    border-color: var(--accent);
  }
  .job.done {
    border-color: rgba(61, 220, 151, 0.35);
  }
  .job.failed {
    border-color: rgba(255, 93, 108, 0.5);
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: 8px;
  }
  .name {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .st {
    font-size: 11px;
    color: var(--text-2);
    white-space: nowrap;
  }
  .done .st {
    color: var(--ok);
  }
  .failed .st {
    color: var(--err);
  }
  .running .st {
    color: #ffb183;
  }
  .meta {
    font-size: 11px;
  }
  .pbar {
    height: 4px;
    border-radius: 4px;
    background: var(--panel-3);
    overflow: hidden;
  }
  .pbar span {
    display: block;
    height: 100%;
    background: linear-gradient(90deg, var(--accent), var(--accent-2));
    transition: width 0.25s;
  }
  .file {
    display: flex;
    justify-content: space-between;
  }
  .file button {
    font-size: 12px;
    padding: 2px 4px;
  }
  .err,
  .note {
    margin: 0;
    font-size: 12px;
    white-space: pre-wrap;
    user-select: text;
  }
  .err {
    color: var(--err);
  }
  .note {
    color: var(--warn);
  }
  .cancel {
    justify-self: start;
    font-size: 12px;
    padding: 3px 8px;
  }
  .clear {
    align-self: flex-start;
    font-size: 12px;
    color: var(--text-3);
  }
</style>
