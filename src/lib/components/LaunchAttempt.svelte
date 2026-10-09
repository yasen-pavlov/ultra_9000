<script lang="ts">
  // The content panel for a launch attempt that has no process yet: "Starting OMP…" while native
  // resolves and spawns the tool, or the failure with Retry / Change… / Dismiss. A failed attempt
  // never leaves a fake item behind; Dismiss removes the row.
  import { run } from "../errors.svelte";
  import { tildePath } from "../format";
  import { itemClose, itemRestart, sheetOpen, type ItemState } from "../native";
  import { workspace } from "../workspace.svelte";
  import ToolTile from "./ToolTile.svelte";

  let { item }: { item: ItemState } = $props();

  const tool = $derived(workspace.tool(item.tool));
  const label = $derived(tool?.label ?? item.tool);
  const folder = $derived(tildePath(item.cwd, workspace.state?.defaultCwd ?? ""));
  const failed = $derived(item.process === "failed");
  let primary = $state<HTMLButtonElement | null>(null);

  // A failed attempt takes focus on its safe primary action; a starting one leaves focus alone so
  // the terminal can take it through the viewport protocol as soon as the process runs.
  $effect(() => {
    void item.id;
    if (failed) primary?.focus();
  });
</script>

<div class="attempt on-panel" role="status">
  <ToolTile
    tool={item.tool}
    model={item.model}
    badge={failed ? "err" : "start"}
    badgeGlyph={failed ? "×" : ""}
    size={30}
  />
  {#if failed}
    <h2>Couldn't start {label}</h2>
    <p class="err selectable">{item.failure ?? "The launch failed before a process existed."}</p>
    <p class="dim">{item.conversation !== null ? `Resume “${item.title}”` : "New"} · {folder} · nothing was started</p>
    <div class="acts">
      <button type="button" class="btn pri" bind:this={primary} onclick={() => run("Retry", () => itemRestart(item.id))}
        >Retry</button
      >
      <button
        type="button"
        class="btn"
        onclick={() => run("Open", () => sheetOpen({ kind: "palette", projectId: item.projectId, mode: "new" }))}
        >Change…</button
      >
      <button type="button" class="btn ghost" onclick={() => run("Dismiss", () => itemClose(item.id, false))}
        >Dismiss</button
      >
    </div>
  {:else}
    <h2>Starting {label}…</h2>
    <p class="dim">{item.conversation !== null ? `Resuming “${item.title}” in ${folder}` : `In ${folder}`}</p>
    <p class="dim">The terminal appears as soon as the process runs.</p>
    <div class="acts">
      <button
        type="button"
        class="btn"
        bind:this={primary}
        onclick={() => run("Cancel", () => itemClose(item.id, false))}>Cancel</button
      >
    </div>
  {/if}
</div>

<style>
  .attempt {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 28px;
    padding-bottom: 80px;
    text-align: center;
    min-height: 0;
  }
  h2 {
    margin: 6px 0 0;
    font-family: var(--display);
    font-size: 20px;
    font-weight: 600;
    color: var(--hi);
  }
  p {
    margin: 0;
    max-width: 560px;
    line-height: 1.5;
  }
  .err {
    color: var(--err-text);
  }
  .dim {
    color: var(--dim);
    font-size: 12.5px;
  }
  .acts {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }
</style>
