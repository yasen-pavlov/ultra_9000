<script lang="ts">
  // The outline (board 31): H2 and H3, the current heading in --hi with a 1 px rule. A 216 px rail
  // beside the prose on wide panels, otherwise a popover opened with `o`. ↑ ↓ move, ↵ goes, Esc
  // returns to the page; clicking goes too.
  import type { DocumentHeading } from "../document/render";

  let {
    entries,
    currentId,
    variant,
    ongo,
    onback,
  }: {
    entries: DocumentHeading[];
    currentId: string | null;
    variant: "rail" | "popover";
    ongo: (heading: DocumentHeading) => void;
    /** Esc: focus goes back to the page (and a popover closes). */
    onback: () => void;
  } = $props();

  let list = $state<HTMLElement | null>(null);

  /** Moves focus into the outline, onto the current heading. */
  export function focus(): void {
    const items = [...(list?.querySelectorAll<HTMLElement>("button") ?? [])];
    const at = Math.max(
      0,
      entries.findIndex((entry) => entry.id === currentId),
    );
    items[at]?.focus();
  }

  export function contains(element: Element | null): boolean {
    return element !== null && list?.contains(element) === true;
  }

  $effect(() => {
    if (variant === "popover") focus();
  });

  function onkeydown(event: KeyboardEvent): void {
    const items = [...(list?.querySelectorAll<HTMLElement>("button") ?? [])];
    const at = items.indexOf(document.activeElement as HTMLElement);
    switch (event.key) {
      case "ArrowDown":
      case "j":
        items[Math.min(items.length - 1, at + 1)]?.focus();
        break;
      case "ArrowUp":
      case "k":
        items[Math.max(0, at - 1)]?.focus();
        break;
      case "Escape":
        onback();
        break;
      case "Enter":
      case " ":
        if (at >= 0) ongo(entries[at]);
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  }
</script>

<nav class="outline {variant}" class:menu={variant === "popover"} aria-label="Outline">
  <!-- Arrow keys move between the heading buttons inside. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <ol bind:this={list} {onkeydown}>
    {#each entries as entry (entry.id)}
      <li class="h{entry.level}">
        <button
          type="button"
          class:cur={entry.id === currentId}
          aria-current={entry.id === currentId ? "location" : undefined}
          onclick={() => ongo(entry)}>{entry.text}</button
        >
      </li>
    {/each}
  </ol>
</nav>

<style>
  .outline ol {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .outline button {
    display: block;
    width: 100%;
    text-align: left;
    font-size: 12.5px;
    line-height: 20px;
    padding: 2px 0 2px 12px;
    color: var(--dim);
    border-left: 1px solid var(--line);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    border-radius: 0;
  }
  .outline .h3 button {
    padding-left: 24px;
  }
  .outline button:hover {
    color: var(--text);
  }
  .outline button.cur {
    color: var(--hi);
    border-left-color: var(--hi);
  }
  .rail {
    position: sticky;
    top: 56px;
    margin-top: var(--doc-top, 56px);
    align-self: flex-start;
    width: 216px;
    flex: none;
    max-height: calc(100vh - 112px);
    overflow-y: auto;
    padding: 0 16px 0 0;
    scrollbar-width: none;
  }
  .popover {
    top: calc(var(--chrome-top, 0px) + 40px);
    right: 12px;
    width: 280px;
    padding: 8px 8px 8px 4px;
  }
</style>
