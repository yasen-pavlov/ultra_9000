<script lang="ts">
  // Close confirmation (board 13): only for a working/needs-you OMP agent, a shell with a foreground
  // job, Neovim, or (opt-in) an unmonitored agent. Default focus is the safe button; Esc cancels;
  // pressing the close shortcut again confirms natively.
  import { clock } from "../clock.svelte";
  import { run } from "../errors.svelte";
  import { closeQuestion } from "../confirm";
  import { modelShort, spokenAge } from "../format";
  import { itemView } from "../items";
  import { itemClose, sheetClose, type CloseReason } from "../native";
  import { workspace } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import ToolTile from "./ToolTile.svelte";

  let { itemId, reason, detail }: { itemId: string; reason: CloseReason; detail: string | null } = $props();

  let keep = $state<HTMLButtonElement | null>(null);

  const item = $derived(workspace.itemsById.get(itemId) ?? null);
  const view = $derived(
    item === null
      ? null
      : itemView(item, { tool: workspace.tool(item.tool), now: clock.now, home: workspace.state?.defaultCwd ?? "" }),
  );
  const label = $derived(item === null ? "" : workspace.toolLabel(item.tool));
  const project = $derived(item === null ? "" : workspace.projectName(item.projectId));
  const closeLabel = $derived(workspace.shortcut("close"));
  const question = $derived(
    closeQuestion({
      item,
      reason,
      detail,
      toolLabel: label,
      toolPath: item === null ? null : (workspace.tool(item.tool)?.path ?? null),
      project,
    }),
  );
  const meta = $derived(
    item === null
      ? ""
      : [
          project,
          label,
          item.model === null ? null : modelShort(item.model),
          reason === "agent-working"
            ? `working for ${spokenAge(item.stateSince, clock.now)}`
            : reason === "agent-needs-you"
              ? `waiting ${spokenAge(item.stateSince, clock.now)}`
              : null,
        ]
          .filter((part) => part !== null && part !== "")
          .join(" · "),
  );

  $effect(() => {
    keep?.focus();
  });

  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    void run("Cancel", () => sheetClose());
  }
</script>

<svelte:window {onkeydown} />

<div class="sheet confirm">
  <div class="card" role="alertdialog" aria-labelledby="confirm-title" aria-describedby="confirm-body">
    <div class="title">
      {#if item !== null && view !== null}
        <span class="on-panel"
          ><ToolTile
            tool={item.tool}
            model={item.model}
            badge={view.badge}
            badgeGlyph={view.badgeGlyph}
            size={30}
          /></span
        >
      {/if}
      <div>
        <h2 id="confirm-title">{question.title}</h2>
        {#if meta !== ""}<div class="meta">{meta}</div>{/if}
      </div>
    </div>
    <div id="confirm-body" class="body">
      {#each question.body as paragraph, index (index)}
        <p class:dim={paragraph.dim}>
          {#each paragraph.parts as part, at (at)}{#if part.strong}<span class="strong">{part.text}</span
              >{:else}{part.text}{/if}{/each}
        </p>
      {/each}
    </div>
    <div class="acts">
      <button
        type="button"
        class="btn"
        bind:this={keep}
        data-sheet-autofocus
        onclick={() => run("Cancel", () => sheetClose())}>Keep running<kbd>Esc</kbd></button
      >
      <button
        type="button"
        class="btn danger"
        disabled={item === null}
        onclick={() => run("Close", () => itemClose(itemId, true))}
      >
        {question.endLabel}{#if closeLabel !== null}<kbd>{closeLabel}</kbd>{/if}
      </button>
    </div>
  </div>
  <div class="hidden-note"><Icon name="sidebar" small />Terminal hidden while this is open · it keeps running</div>
</div>

<style>
  .card {
    max-width: 680px;
    gap: 16px;
  }
  .title {
    display: flex;
    gap: 12px;
    align-items: center;
  }
  /* Integer line (26 + 18 = 44 beside the 30 px tile) keeps the tile on whole pixels. */
  .meta {
    font-size: 12.5px;
    line-height: 18px;
    color: var(--dim);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .dim {
    color: var(--dim);
  }
  .acts {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 6px;
  }
</style>
