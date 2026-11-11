<script lang="ts">
  import { untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { openInEditor, reveal, copyForDiscord, toast } from "../lib/store.svelte";
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
  // Клипы (повторы и нарезки) и целые записи лежат в разных папках и показываются раздельно.
  let kind = $state<"clips" | "recordings">("clips");

  function show(k: "clips" | "recordings") {
    if (k === kind) return;
    kind = k;
    loaded = false;
    clips = [];
    void refresh();
  }

  async function refresh() {
    try {
      clips = await invoke<ClipFile[]>("list_clips", { limit: 300, kind });
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
      // Сначала то, что сейчас на экране (с учётом поиска), остальное следом.
      const shown = new Set(visible.map((c) => c.path));
      const todo = clips.filter((c) => !previews[c.path]).sort((a, b) => Number(shown.has(b.path)) - Number(shown.has(a.path)));
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
    untrack(() => void refresh());
    let un: (() => void) | undefined;
    listen<{ kind: string }>("clip-saved", (e) => {
      // Сохранили запись, пока открыты клипы (или наоборот) — переключаемся на её раздел.
      const k = e.payload?.kind === "recording" ? "recordings" : "clips";
      if (k !== kind) show(k);
      else void refresh();
    }).then((u) => (un = u));
    // Автоочистка убрала старые клипы.
    let un2: (() => void) | undefined;
    listen("clips-changed", () => refresh()).then((u) => (un2 = u));
    return () => {
      un?.();
      un2?.();
    };
  });

  // ---------- Поиск и группировка ----------

  type Grouping = "date" | "game" | "none";
  let query = $state("");
  let grouping = $state<Grouping>("date");
  try {
    const g = localStorage.getItem("gallery.grouping");
    if (g === "date" || g === "game" || g === "none") grouping = g;
  } catch {}
  function setGrouping(g: Grouping) {
    grouping = g;
    try {
      localStorage.setItem("gallery.grouping", g);
    } catch {}
  }

  const STAMP = /^(.*?)_(\d{4})-(\d{2})-(\d{2})_(\d{2})-(\d{2})-(\d{2})(.*)$/;

  const visible = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return clips;
    return clips.filter((c) => c.name.toLowerCase().includes(q) || title(c.name).toLowerCase().includes(q));
  });

  /** Игра или программа из имени файла; у переименованных клипов её нет. */
  function gameOf(name: string) {
    const m = name.replace(/\.[^.]+$/, "").match(STAMP);
    return m ? m[1].replace(/_/g, " ").trim() || "Без названия" : "Свои названия";
  }

  function dateGroup(ms: number) {
    const d = new Date(ms);
    const now = new Date();
    const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const diff = Math.round((day(now) - day(d)) / 86_400_000);
    if (diff <= 0) return "Сегодня";
    if (diff === 1) return "Вчера";
    if (diff < 7) return "На этой неделе";
    const label = d.toLocaleDateString("ru-RU", { month: "long", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" });
    return label.charAt(0).toUpperCase() + label.slice(1);
  }

  // Клипы уже отсортированы от новых к старым, поэтому группы по дате идут в верном порядке сами.
  const groups = $derived.by(() => {
    if (grouping === "none") return [{ label: "", items: visible }];
    const map = new Map<string, ClipFile[]>();
    for (const c of visible) {
      const k = grouping === "date" ? dateGroup(c.modified) : gameOf(c.name);
      (map.get(k) ?? map.set(k, []).get(k)!).push(c);
    }
    const list = [...map].map(([label, items]) => ({ label, items }));
    // По играм: сначала те, где больше клипов.
    if (grouping === "game") list.sort((a, b) => b.items.length - a.items.length);
    return list;
  });

  // ---------- Переименование и удаление ----------

  let editing = $state<string | null>(null);
  let draft = $state("");

  function startRename(c: ClipFile) {
    editing = c.path;
    draft = c.name.replace(/\.[^.]+$/, "");
  }

  async function commitRename(c: ClipFile) {
    if (editing !== c.path) return;
    editing = null;
    const name = draft.trim();
    if (!name || name === c.name.replace(/\.[^.]+$/, "")) return;
    try {
      const path = await invoke<string>("rename_clip", { path: c.path, name });
      // Превью привязано к пути — переносим, чтобы плитка не моргала.
      if (previews[c.path]) previews[path] = previews[c.path];
      c.path = path;
      c.name = path.split(/[\\/]/).pop() ?? c.name;
    } catch (e) {
      toast(String(e));
    }
  }

  function renameKey(e: KeyboardEvent, c: ClipFile) {
    e.stopPropagation();
    if (e.key === "Enter") void commitRename(c);
    else if (e.key === "Escape") editing = null;
  }

  async function remove(c: ClipFile) {
    try {
      await invoke("delete_clip", { path: c.path });
      clips = clips.filter((x) => x.path !== c.path);
      toast("Клип в корзине. Вернуть его можно из корзины Windows.");
    } catch (e) {
      toast(String(e));
    }
  }

  function focusSelect(el: HTMLInputElement) {
    el.focus();
    el.select();
  }

  // Название без даты-штампа: «ZenlessZoneZero_2026-11-06_13-20-37» → «ZenlessZoneZero».
  function title(name: string) {
    const stem = name.replace(/\.[^.]+$/, "");
    const m = stem.match(STAMP);
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
    <div class="tabs" role="tablist">
      <button role="tab" class="tab" class:active={kind === "clips"} aria-selected={kind === "clips"} onclick={() => show("clips")}>Клипы</button>
      <button role="tab" class="tab" class:active={kind === "recordings"} aria-selected={kind === "recordings"} onclick={() => show("recordings")}>Записи</button>
    </div>
    <div class="tools">
      <label class="search">
        <Icon name="search" size={14} />
        <input type="search" placeholder="Поиск по названию" bind:value={query} onkeydown={(e) => e.stopPropagation()} />
      </label>
      <select value={grouping} onchange={(e) => setGrouping(e.currentTarget.value as Grouping)} title="Как разложить клипы">
        <option value="date">По дате</option>
        <option value="game">По играм</option>
        <option value="none">Одним списком</option>
      </select>
      <button class="ghost" onclick={() => invoke("open_clips_folder", { kind })}><Icon name="folder" size={14} /> Открыть папку</button>
    </div>
  </header>

  {#if loaded && !clips.length}
    <p class="empty">
      {kind === "clips"
        ? "Здесь появятся повторы и нарезанные клипы. Включите буфер и нажмите горячую клавишу сохранения во время игры."
        : "Здесь появятся целые записи. Запись начинается и останавливается горячей клавишей записи."}
    </p>
  {:else if loaded && !visible.length}
    <p class="empty">По запросу «{query.trim()}» ничего не нашлось.</p>
  {:else}
    {#each groups as g (g.label)}
      {#if g.label}
        <h4 class="group">{g.label} <span class="n">{g.items.length}</span></h4>
      {/if}
      <div class="grid">
        {#each g.items as c (c.path)}
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
                {#if editing === c.path}
                  <input
                    class="rename"
                    bind:value={draft}
                    use:focusSelect
                    onkeydown={(e) => renameKey(e, c)}
                    onblur={() => commitRename(c)}
                    maxlength="120"
                    aria-label="Новое название клипа"
                  />
                {:else}
                  <span class="name" title="{c.name}. Двойной щелчок переименовывает" ondblclick={() => startRename(c)} role="presentation">{title(c.name)}</span>
                {/if}
                <span class="meta">{when(c.modified)}, {fmtBytes(c.sizeBytes)}</span>
              </div>
              <div class="acts">
                <button class="ghost icon" title="Скопировать файл, чтобы вставить в Discord через Ctrl+V" onclick={() => copyForDiscord(c.path)}><Icon name="copy" size={14} /></button>
                <button class="ghost icon" title="Переименовать" onclick={() => startRename(c)}><Icon name="pencil" size={14} /></button>
                <button class="ghost icon" title="Показать в проводнике" onclick={() => reveal(c.path)}><Icon name="folder" size={14} /></button>
                <button class="ghost icon del" title="Удалить в корзину" onclick={() => remove(c)}><Icon name="trash" size={14} /></button>
              </div>
            </div>
          </article>
        {/each}
      </div>
    {/each}
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
  .tabs {
    display: flex;
    gap: 18px;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text-3);
  }
  .search:focus-within {
    border-color: var(--line-2);
    color: var(--text);
  }
  .search input {
    width: 170px;
    border: none;
    background: none;
    padding: 5px 0;
    outline: none;
  }
  .tools select {
    padding: 5px 8px;
  }
  .group {
    margin: 6px 0 -2px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-2);
  }
  .group .n {
    margin-left: 4px;
    font-weight: 400;
    color: var(--text-3);
  }
  .acts {
    display: flex;
    flex-shrink: 0;
  }
  /* Действия с клипом видны при наведении на плитку или когда фокус внутри неё. */
  .tile .acts {
    opacity: 0.35;
  }
  .tile:hover .acts,
  .tile:focus-within .acts {
    opacity: 1;
  }
  .del:hover:not(:disabled) {
    color: var(--err);
  }
  .rename {
    width: 100%;
    padding: 2px 6px;
    font: inherit;
    font-weight: 500;
  }
  .tab {
    padding: 2px 0 6px;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    color: var(--text-3);
    font-weight: 600;
    font-size: 13px;
  }
  .tab:hover:not(:disabled) {
    background: none;
    color: var(--text);
  }
  .tab.active {
    color: var(--text);
    border-bottom-color: var(--cue, #f5c518);
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
