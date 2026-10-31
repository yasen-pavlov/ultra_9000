<script lang="ts" module>
  export type DocumentMenuAction = "neovim" | "reveal" | "copy-path" | "follow" | "outline" | "find" | "reload";
</script>

<script lang="ts">
  // The document's actions (board 34): every action has a key and a line here. The app's existing
  // .menu, opened by `.` or the ⋯ at the top right. ↑ ↓ move, ↵ runs, Esc closes.
  let {
    follow,
    onaction,
    onclose,
  }: {
    follow: boolean;
    onaction: (action: DocumentMenuAction) => void;
    onclose: () => void;
  } = $props();

  const entries = $derived<{ action: DocumentMenuAction; label: string; key: string }[]>([
    { action: "neovim", label: "Open in Neovim", key: "e" },
    { action: "reveal", label: "Reveal in file manager", key: "v" },
    { action: "copy-path", label: "Copy path", key: "y" },
    { action: "follow", label: follow ? "Stop following" : "Follow", key: "f" },
    { action: "outline", label: "Outline", key: "o" },
    { action: "find", label: "Find in document", key: "/" },
    { action: "reload", label: "Reload", key: "r" },
  ]);

  let cursor = $state(0);
  let list = $state<HTMLElement | null>(null);

  $effect(() => {
    list?.querySelector<HTMLElement>(".mi")?.focus();
  });

  function focusAt(index: number): void {
    cursor = (index + entries.length) % entries.length;
    list?.querySelectorAll<HTMLElement>(".mi")[cursor]?.focus();
  }

  function onkeydown(event: KeyboardEvent): void {
    switch (event.key) {
      case "ArrowDown":
        focusAt(cursor + 1);
        break;
      case "ArrowUp":
        focusAt(cursor - 1);
        break;
      case "Home":
        focusAt(0);
        break;
      case "End":
        focusAt(entries.length - 1);
        break;
      case "Escape":
        onclose();
        break;
      case "Tab":
        onclose();
        return;
      default: {
        const entry = entries.find((candidate) => candidate.key === event.key);
        if (entry === undefined || event.ctrlKey || event.metaKey || event.altKey) return;
        onaction(entry.action);
      }
    }
    event.preventDefault();
    event.stopPropagation();
  }
</script>

<div
  class="menu doc-menu"
  role="menu"
  tabindex="-1"
  aria-label="Document actions"
  bind:this={list}
  {onkeydown}
  data-doc-overlay
>
  {#each entries as entry, index (entry.action)}
    <button
      type="button"
      class="mi"
      class:sel={cursor === index}
      role="menuitem"
      tabindex={cursor === index ? 0 : -1}
      onfocus={() => (cursor = index)}
      onmouseenter={() => (cursor = index)}
      onclick={() => onaction(entry.action)}>{entry.label}<span class="s"><kbd>{entry.key}</kbd></span></button
    >
  {/each}
</div>

<style>
  .doc-menu {
    top: 40px;
    right: 12px;
    width: 250px;
    max-height: none;
  }
</style>
