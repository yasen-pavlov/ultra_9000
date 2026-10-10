<script lang="ts">
  // Quit confirmation (board 15): lists everything that ends, folded groups included, with the
  // consequence per kind. Default focus Cancel; "Hide instead" ends nothing.
  import { clock } from "../clock.svelte";
  import { run } from "../errors.svelte";
  import { itemView } from "../items";
  import { appHide, appQuit, sheetClose, type ItemState } from "../native";
  import { workspace } from "../workspace.svelte";
  import ToolTile from "./ToolTile.svelte";

  let { itemIds }: { itemIds: string[] } = $props();

  let cancel = $state<HTMLButtonElement | null>(null);

  const items = $derived(
    itemIds.flatMap((id) => {
      const item = workspace.itemsById.get(id);
      return item === undefined ? [] : [item];
    }),
  );
  const count = $derived(itemIds.length);
  const folded = $derived.by(() => {
    const foldedIds = new Set(workspace.state?.ui.foldedProjectIds ?? []);
    return workspace.groups
      .filter((group) => foldedIds.has(group.project.id))
      .map((group) => ({
        name: group.project.name,
        count: group.items.filter((item) => itemIds.includes(item.id)).length,
      }))
      .filter((group) => group.count > 0);
  });
  const hideLabel = $derived(workspace.shortcut("hide"));

  /** What ending means for this item; `lost` marks consequences that destroy unsaved work. */
  function consequence(item: ItemState): { text: string; lost: boolean } {
    const tool = workspace.tool(item.tool);
    if (item.process !== "running") return { text: "already ended · row removed", lost: false };
    if (item.kind === "agent") {
      if (item.activity === "needs-you") {
        return {
          text: `waiting for ${item.attention === "question" ? "an answer" : item.attention === "approval" ? "approval" : "you"} — that turn is lost`,
          lost: true,
        };
      }
      if (item.activity === "working") return { text: "working — this turn is lost", lost: true };
      if (item.activity === "on-job") {
        const labels = item.backgroundJobs.map((job) => job.label).join(", ");
        return { text: `waiting on its jobs (${labels}) — ending stops them`, lost: true };
      }
      if (tool?.status === "omp") return { text: "saved · resumable", lost: false };
      return { text: `${tool?.label ?? item.tool} keeps its own save`, lost: false };
    }
    if (item.tool === "neovim") return { text: "Neovim ends · unsaved buffers lost", lost: true };
    return { text: `${tool?.label.toLowerCase() ?? "terminal"} ends · screen gone`, lost: false };
  }

  $effect(() => {
    cancel?.focus();
  });

  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    void run("Cancel", () => sheetClose());
  }
</script>

<svelte:window {onkeydown} />

<div class="sheet quit">
  <div class="card" role="alertdialog" aria-labelledby="quit-title" aria-describedby="quit-body">
    <h2 id="quit-title">Quit Ultra 9000 and end {count} running {count === 1 ? "item" : "items"}?</h2>
    <p id="quit-body">
      Every process started here ends now{#if folded.length > 0}, including {#each folded as group, index (index)}{index >
          0
            ? index === folded.length - 1
              ? " and "
              : ", "
            : ""}the {group.count} in folded <span class="strong">{group.name}</span>{/each}{/if}. Agent conversations
      stay saved and can be resumed after the next start. Shell and Neovim screens are not kept. This can't be undone.
    </p>
    <div class="list scroll">
      {#each items as item (item.id)}
        {@const view = itemView(item, {
          tool: workspace.tool(item.tool),
          now: clock.now,
          home: workspace.state?.defaultCwd ?? "",
        })}
        {@const outcome = consequence(item)}
        <div class="pr">
          <ToolTile tool={item.tool} model={item.model} badge={view.badge} badgeGlyph={view.badgeGlyph} />
          <span class="tx"
            ><span class="t"
              >{view.title}{workspace.disambiguation.get(item.id) ?? ""}
              <span class="dis">· {workspace.projectName(item.projectId)}</span></span
            ></span
          >
          <span class="act" class:lost={outcome.lost}>{outcome.text}</span>
        </div>
      {/each}
    </div>
    <div class="acts">
      <button type="button" class="btn ghost" onclick={() => run("Hide", () => appHide())}
        >Hide instead{#if hideLabel !== null}<kbd>{hideLabel}</kbd>{/if}</button
      >
      <span class="grow"></span>
      <button
        type="button"
        class="btn"
        bind:this={cancel}
        data-sheet-autofocus
        onclick={() => run("Cancel", () => sheetClose())}>Cancel<kbd>Esc</kbd></button
      >
      <button type="button" class="btn danger solid" onclick={() => run("Quit", () => appQuit())}
        >Quit and end {count}</button
      >
    </div>
  </div>
</div>

<style>
  .card {
    max-width: 680px;
    gap: 12px;
  }
  /* An integer line (not 1.5 × 13 px) keeps the list below, and its tiles, on whole pixels. */
  #quit-body {
    line-height: 20px;
  }
  /* The list's own surface is the tiles' row background (provider plate, badge ring). */
  .list {
    --row-bg: #1d1d1d;
    border: 1px solid var(--line);
    border-radius: 9px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    background: var(--row-bg);
    max-height: 50vh;
  }
  .list .pr {
    min-height: 34px;
  }
  .act {
    color: var(--dim);
  }
  .act.lost {
    color: #ffb3c0;
  }
  .acts {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 4px;
  }
  .grow {
    flex: 1;
  }
</style>
