<script lang="ts">
  // The shortcut map for this platform, rendered from the native snapshot (keys.toml overrides
  // included), plus the keys the palette, sidebar and confirmations handle themselves.
  import { run } from "../errors.svelte";
  import { settingsOpenFile, sheetClose, type ShortcutAction } from "../native";
  import { settings } from "../settings.svelte";
  import { workspace } from "../workspace.svelte";

  const GROUPS: { title: string; rows: { action: ShortcutAction | "items"; label: string }[] }[] = [
    {
      title: "Open and start",
      rows: [
        { action: "palette", label: "Open: switch, resume or start" },
        { action: "palette-project", label: "Open, scoped to this project" },
        { action: "new-agent", label: "New agent here (project default tool)" },
        { action: "new-terminal", label: "New terminal here, below the selected item" },
        { action: "new-neovim", label: "New Neovim here (on a document: that file)" },
        { action: "documents", label: "Documents in this project" },
      ],
    },
    {
      title: "Items",
      rows: [
        { action: "items", label: "Item 1 … 9 (visible order, folded projects skipped)" },
        { action: "item-previous", label: "Previous item" },
        { action: "item-next", label: "Next item" },
        { action: "project-previous", label: "Previous project" },
        { action: "project-next", label: "Next project" },
        { action: "jump-waiting", label: "Jump to what's waiting (repeat cycles)" },
        { action: "close", label: "Close item (press again to confirm)" },
      ],
    },
    {
      title: "Window",
      rows: [
        { action: "focus-sidebar", label: "Focus the sidebar" },
        { action: "toggle-sidebar", label: "Expand or collapse the sidebar" },
        { action: "settings", label: "Settings" },
        { action: "shortcuts", label: "Show shortcuts" },
        { action: "hide", label: "Hide window (nothing ends)" },
        { action: "quit", label: "Quit (lists what it ends)" },
      ],
    },
  ];

  const LOCAL: { title: string; rows: { keys: string[]; label: string }[] }[] = [
    {
      title: "Inside the palette",
      rows: [
        { keys: ["↑", "↓"], label: "Move" },
        { keys: ["↵"], label: "Open the row: switch, view, resume or start" },
        { keys: ["Ctrl+↵"], label: "Start new, whatever is selected (on a document: Neovim)" },
        { keys: ["Tab"], label: "Walk the launch line: tool, project, folder (menus open with ↓)" },
        { keys: ["Backspace"], label: "On an empty query: remove the project scope" },
        { keys: ["Esc"], label: "Close an open menu, then the palette" },
      ],
    },
    {
      title: "Inside a document",
      rows: [
        { keys: ["j", "k"], label: "Scroll three lines (k turns follow off)" },
        { keys: ["d", "u"], label: "Half a page" },
        { keys: ["Space", "Shift+Space"], label: "A page, less one line" },
        { keys: ["g g", "G"], label: "Top, end (G while live follows again)" },
        { keys: ["[", "]"], label: "Previous, next heading" },
        { keys: ["f"], label: "Follow the end on or off" },
        { keys: ["o"], label: "Outline" },
        { keys: ["/"], label: "Find in document (n, N step)" },
        { keys: ["e"], label: "Open in Neovim" },
        { keys: ["v"], label: "Reveal in file manager" },
        { keys: ["y"], label: "Copy path" },
        { keys: ["r"], label: "Reload" },
        { keys: ["."], label: "Actions menu" },
      ],
    },
    {
      title: "Sidebar, after focusing it",
      rows: [
        { keys: ["↑", "↓"], label: "Move" },
        { keys: ["↵"], label: "Select the item, or fold the project" },
        { keys: ["←", "→"], label: "Fold, unfold" },
        { keys: ["Delete"], label: "Close the item" },
        { keys: ["Esc"], label: "Back to the terminal or document" },
      ],
    },
    {
      title: "Confirmations",
      rows: [
        { keys: ["Esc"], label: "Always the safe choice" },
        { keys: ["Tab"], label: "Reach the destructive button" },
      ],
    },
  ];

  let done = $state<HTMLButtonElement | null>(null);

  const keysFile = $derived(settings.state?.files.find((file) => file.name === "keys.toml") ?? null);

  function itemRange(): string | null {
    const first = workspace.shortcut("item-1");
    const last = workspace.shortcut("item-9");
    if (first === null && last === null) return null;
    if (first === null || last === null) return first ?? last;
    return `${first} … ${last.slice(last.lastIndexOf("+") + 1)}`;
  }

  $effect(() => {
    done?.focus();
  });

  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    void run("Close", () => sheetClose());
  }
</script>

<svelte:window {onkeydown} />

<div class="sheet shortcuts">
  <div class="card">
    <div class="head">
      <h2>Keyboard shortcuts</h2>
      <button
        type="button"
        class="btn sm ghost done"
        bind:this={done}
        data-sheet-autofocus
        onclick={() => run("Close", () => sheetClose())}>Done<kbd>Esc</kbd></button
      >
    </div>
    <p class="lede">
      App shortcuts work while the terminal has focus and never use a chord the terminal, OMP, Neovim or the compositor
      owns. Override them in
      <code>keys.toml</code>; conflicts show up in Settings.
      {#if keysFile !== null}
        <button
          type="button"
          class="btn xs"
          onclick={() => run("Open keys.toml", () => settingsOpenFile(keysFile.path))}
        >
          {keysFile.exists ? "Open keys.toml" : "Create keys.toml"}
        </button>
      {/if}
    </p>
    <div class="cols scroll">
      <div class="col">
        {#each GROUPS as group (group.title)}
          <div class="sec">{group.title}</div>
          {#each group.rows as row (row.action)}
            {@const label = row.action === "items" ? itemRange() : workspace.shortcut(row.action)}
            <div class="kr">
              <span class="l">{row.label}</span>
              {#if label === null}<span class="unbound">unbound</span>{:else}<kbd>{label}</kbd>{/if}
            </div>
          {/each}
        {/each}
      </div>
      <div class="col">
        {#each LOCAL as group (group.title)}
          <div class="sec">{group.title}</div>
          {#each group.rows as row (row.label)}
            <div class="kr">
              <span class="l">{row.label}</span>
              <span class="ks"
                >{#each row.keys as key (key)}<kbd>{key}</kbd>{/each}</span
              >
            </div>
          {/each}
        {/each}
      </div>
    </div>
  </div>
</div>

<style>
  .card {
    max-width: 860px;
    flex: 1;
    gap: 10px;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 12px;
  }
  .done {
    margin-left: auto;
  }
  .lede {
    color: var(--dim);
    font-size: 12.5px;
  }
  code {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--text);
  }
  .cols {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 0 28px;
    align-content: start;
  }
  .kr {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 30px;
    border-bottom: 1px solid var(--line);
    padding: 0 4px;
  }
  .l {
    flex: 1;
    color: var(--hi);
    font-size: 13px;
  }
  .ks {
    display: flex;
    gap: 4px;
  }
  .unbound {
    color: var(--dim);
    font-size: 11.5px;
  }
</style>
