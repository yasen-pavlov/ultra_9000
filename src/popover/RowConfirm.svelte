<script lang="ts">
  // The close question in place of its row (board 04), worded as the app's close sheet. Focus
  // starts on Keep running; w or ↵ on End confirms; ↑ ↓ or Esc keep the item running.
  import { clock } from "../lib/clock.svelte";
  import ToolTile from "../lib/components/ToolTile.svelte";
  import { closeQuestion } from "../lib/confirm";
  import { itemView } from "../lib/items";
  import type { CloseReason, ItemState } from "../lib/native";
  import { workspace } from "../lib/workspace.svelte";

  let { item, reason, onkeep, onend }: { item: ItemState; reason: CloseReason; onkeep: () => void; onend: () => void } =
    $props();

  let keep = $state<HTMLButtonElement | null>(null);
  const uid = $props.id();

  const view = $derived(
    itemView(item, { tool: workspace.tool(item.tool), now: clock.now, home: workspace.state?.defaultCwd ?? "" }),
  );
  const question = $derived(
    closeQuestion({
      item,
      reason,
      detail: item.foregroundJob?.command ?? null,
      toolLabel: workspace.toolLabel(item.tool),
      toolPath: workspace.tool(item.tool)?.path ?? null,
      project: workspace.projectName(item.projectId),
    }),
  );

  $effect(() => {
    keep?.focus();
  });
</script>

<!-- Inline in the list, not modal: the rest of the popover stays usable. -->
<div class="conf" role="alertdialog" aria-labelledby="{uid}-title" aria-describedby="{uid}-why" data-item-id={item.id}>
  <div class="top">
    <span class="tile-at"
      ><ToolTile tool={item.tool} model={item.model} badge={view.badge} badgeGlyph={view.badgeGlyph} /></span
    >
    <span class="tx">
      <span class="q" id="{uid}-title">{question.title}</span>
      <span class="why" id="{uid}-why">
        {#each question.body as paragraph, index (index)}<span class="para"
            >{#each paragraph.parts as part, at (at)}{#if part.strong}<b>{part.text}</b
                >{:else}{part.text}{/if}{/each}</span
          >{/each}
      </span>
    </span>
  </div>
  <div class="btns">
    <button type="button" class="btn sm" bind:this={keep} onclick={onkeep}>Keep running</button>
    <button type="button" class="btn sm danger solid" onclick={onend}>{question.endLabel}<kbd>w</kbd></button>
  </div>
</div>

<style>
  .conf {
    --row-bg: var(--sel);
    background: var(--sel);
    border-radius: 8px;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    box-shadow: inset 0 0 0 2px var(--blue);
    flex: none;
  }
  .top {
    display: flex;
    gap: 9px;
    align-items: flex-start;
  }
  .tile-at {
    margin-top: 1px;
    flex: none;
  }
  .tx {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .q {
    font-weight: 600;
    font-size: 13px;
    line-height: 18px;
    color: var(--hi);
    overflow-wrap: anywhere;
  }
  .why {
    font-size: 11.5px;
    line-height: 16px;
    color: var(--dim);
  }
  .why b {
    color: var(--hi);
    font-weight: 600;
  }
  .btns {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
  }
  .btn.danger.solid kbd {
    color: #22000a;
    border-color: rgba(0, 0, 0, 0.3);
    background: transparent;
  }
  /* The sheet's paragraphs run on as one block here. */
  .para + .para::before {
    content: " ";
  }
</style>
