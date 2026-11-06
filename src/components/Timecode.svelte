<script lang="ts">
  // Таймкод как счётчик на пульте: каждая цифра в своей ячейке фиксированной ширины,
  // поэтому широкий Unbounded не «прыгает» при смене цифр.
  let { value, size = 15, tone = "" }: { value: string; size?: number; tone?: string } = $props();
  const chars = $derived(value.split(""));
</script>

<span class="tc {tone}" style="font-size:{size}px" aria-label={value}>
  {#each chars as c, i (i)}
    <span class:sep={!/\d/.test(c)}>{c}</span>
  {/each}
</span>

<style>
  .tc {
    font-family: var(--tc);
    font-weight: 500;
    display: inline-flex;
    line-height: 1;
    letter-spacing: 0;
    white-space: nowrap;
  }
  .tc span {
    display: inline-block;
    width: 0.74em;
    text-align: center;
  }
  .tc span.sep {
    width: 0.34em;
    color: var(--text-3);
  }
  .cue {
    color: var(--cue-text);
  }
  .dim {
    color: var(--text-3);
  }
  .tally {
    color: var(--tally);
  }
</style>
