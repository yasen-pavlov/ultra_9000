<script lang="ts">
  // Find in document (board 34): the palette's field style, top right of the reader. ↵ / Shift+↵
  // step (n / N in the page do too), Esc closes and keeps the reader at the current match. Owns the
  // keyboard while focused: the page's reading keys are inert.
  import Icon from "./Icon.svelte";

  let {
    query = $bindable(),
    current,
    total,
    onstep,
    onclose,
  }: {
    query: string;
    /** 0-based index of the current match; meaningless when total is 0. */
    current: number;
    total: number;
    onstep: (direction: 1 | -1) => void;
    onclose: () => void;
  } = $props();

  let input = $state<HTMLInputElement | null>(null);

  export function focus(): void {
    input?.focus();
    input?.select();
  }

  $effect(() => {
    input?.focus();
  });

  function onkeydown(event: KeyboardEvent): void {
    if (event.key === "Enter") onstep(event.shiftKey ? -1 : 1);
    else if (event.key === "Escape") onclose();
    else return;
    event.preventDefault();
    event.stopPropagation();
  }
</script>

<div class="find field" role="search">
  <Icon name="search" small />
  <input
    bind:this={input}
    bind:value={query}
    {onkeydown}
    spellcheck="false"
    autocomplete="off"
    aria-label="Find in document"
    placeholder="Find in document"
  />
  {#if query !== ""}
    <span class="count" aria-live="polite">{total === 0 ? "no matches" : `${current + 1} of ${total}`}</span>
  {/if}
</div>

<style>
  .find {
    position: absolute;
    top: 12px;
    right: 48px;
    z-index: 4;
    width: 280px;
    max-width: calc(100% - 64px);
    background: #262626;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
  }
  .count {
    flex: none;
    font-size: 11.5px;
    color: var(--dim);
    white-space: nowrap;
  }
</style>
