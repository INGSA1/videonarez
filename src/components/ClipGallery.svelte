<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { openInEditor, reveal, copyForDiscord } from "../lib/store.svelte";
  import { fmtBytes, fmtTime } from "../lib/format";
  import Icon from "./Icon.svelte";
  import Timecode from "./Timecode.svelte";

  interface ClipFile {
    path: string;
    name: string;
    sizeBytes: number;
    modified: number;
  }
  interface Preview {
    thumb: string | null;
    duration: number;
  }

  let clips = $state<ClipFile[]>([]);
  let previews = $state<Record<string, Preview>>({});
  let loaded = $state(false);

  async function refresh() {
    try {
      clips = await invoke<ClipFile[]>("list_clips", { limit: 24 });
    } catch {
      clips = [];
    }
    loaded = true;
    void loadPreviews();
  }

  // Превью грузятся по три параллельно — ffmpeg декодирует только один ключевой кадр на файл.
  let loading = false;
  async function loadPreviews() {
    if (loading) return;
    loading = true;
    try {
      const todo = clips.filter((c) => !previews[c.path]);
      for (let i = 0; i < todo.length; i += 3) {
        await Promise.all(
          todo.slice(i, i + 3).map(async (c) => {
            try {
              previews[c.path] = await invoke<Preview>("clip_preview", { path: c.path });
            } catch {
              previews[c.path] = { thumb: null, duration: 0 };
            }
          }),
        );
      }
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void refresh();
    let un: (() => void) | undefined;
    listen("clip-saved", () => refresh()).then((u) => (un = u));
    return () => un?.();
  });

  // Название без даты-штампа: «ZenlessZoneZero_2026-11-06_13-20-37» → «ZenlessZoneZero».
  function title(name: string) {
    const stem = name.replace(/\.[^.]+$/, "");
    const m = stem.match(/^(.*?)_(\d{4})-(\d{2})-(\d{2})_(\d{2})-(\d{2})-(\d{2})(.*)$/);
    return m ? (m[1] + (m[8] ?? "")).replace(/_/g, " ").trim() || stem : stem.replace(/_/g, " ");
  }

  function when(ms: number) {
    const d = new Date(ms);
    const now = new Date();
    const time = d.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
    const sameDay = d.toDateString() === now.toDateString();
    const y = new Date(now);
    y.setDate(now.getDate() - 1);
    if (sameDay) return `сегодня в ${time}`;
    if (d.toDateString() === y.toDateString()) return `вчера в ${time}`;
    return `${d.toLocaleDateString("ru-RU", { day: "numeric", month: "long" })}, ${time}`;
  }
</script>

<section class="gallery">
  <header>
    <h3 class="h">Клипы</h3>
    <button class="ghost" onclick={() => invoke("open_clips_folder")}><Icon name="folder" size={14} /> Открыть папку</button>
  </header>

  {#if loaded && !clips.length}
    <p class="empty">
      Здесь появятся сохранённые повторы и записи. Включите буфер и нажмите горячую клавишу сохранения во время игры.
    </p>
  {:else}
    <div class="grid">
      {#each clips as c (c.path)}
        {@const p = previews[c.path]}
        <article class="tile">
          <button class="frame" onclick={() => openInEditor(c.path)} title="Открыть в редакторе">
            {#if p?.thumb}
              <img src={p.thumb} alt="" draggable="false" />
            {:else}
              <span class="noimg">{p ? "Нет кадра" : ""}</span>
            {/if}
            {#if p?.duration}
              <span class="len"><Timecode value={fmtTime(p.duration, 0)} size={11} /></span>
            {/if}
            <span class="hover"><Icon name="scissors" size={16} /> Нарезать</span>
          </button>
          <div class="info">
            <div class="text">
              <span class="name" title={c.name}>{title(c.name)}</span>
              <span class="meta">{when(c.modified)}, {fmtBytes(c.sizeBytes)}</span>
            </div>
            <button class="ghost icon" title="Скопировать файл, чтобы вставить в Discord через Ctrl+V" onclick={() => copyForDiscord(c.path)}><Icon name="copy" size={14} /></button>
            <button class="ghost icon" title="Показать в проводнике" onclick={() => reveal(c.path)}><Icon name="folder" size={14} /></button>
          </div>
        </article>
      {/each}
    </div>
  {/if}
</section>

<style>
  .gallery {
    display: grid;
    gap: 12px;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .empty {
    margin: 0;
    color: var(--text-3);
    max-width: 60ch;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
    gap: 18px 14px;
  }
  .tile {
    display: grid;
    gap: 8px;
    min-width: 0;
  }
  .frame {
    position: relative;
    aspect-ratio: 16 / 9;
    padding: 0;
    border-radius: var(--radius-lg);
    overflow: hidden;
    background: #111214;
    display: block;
    width: 100%;
  }
  .frame:hover:not(:disabled) {
    background: #111214;
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .noimg {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--text-3);
    font-size: 12px;
  }
  .len {
    position: absolute;
    right: 6px;
    bottom: 6px;
    padding: 3px 5px;
    border-radius: 3px;
    background: rgba(17, 18, 20, 0.82);
    color: var(--text);
  }
  .hover {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    background: rgba(17, 18, 20, 0.55);
    color: var(--text);
    font-weight: 600;
    opacity: 0;
  }
  .frame:hover .hover,
  .frame:focus-visible .hover {
    opacity: 1;
  }
  .info {
    display: flex;
    align-items: flex-start;
    gap: 6px;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 1px;
  }
  .name {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    font-size: 12px;
    color: var(--text-3);
  }
</style>
