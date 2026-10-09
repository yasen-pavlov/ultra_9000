<script lang="ts">
  // A project header (folds on click; keeps an aggregate while folded) and its running items.
  import { clock } from "../clock.svelte";
  import { tildePath } from "../format";
  import { itemView } from "../items";
  import type { ItemState } from "../native";
  import { settings } from "../settings.svelte";
  import { workspace, type ProjectGroup } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import SidebarRow from "./SidebarRow.svelte";

  let {
    group,
    roveKey,
    onfold,
    onnew,
    onselect,
    onclose,
  }: {
    group: ProjectGroup;
    roveKey: string | null;
    onfold: () => void;
    onnew: () => void;
    onselect: (item: ItemState) => void;
    onclose: (item: ItemState) => void;
  } = $props();

  const project = $derived(group.project);
  const home = $derived(workspace.state?.defaultCwd ?? "");
  const needsYou = $derived(group.items.filter((item) => item.activity === "needs-you").length);
  const unread = $derived(group.items.filter((item) => item.activity === "your-turn" && item.unread).length);
  const compact = $derived(settings.density === "compact");
</script>

<div class="proj" class:folded={group.folded}>
  <button
    type="button"
    class="head"
    data-rove="head:{project.id}"
    tabindex={roveKey === `head:${project.id}` ? 0 : -1}
    aria-expanded={!group.folded}
    aria-label="{project.name}, {group.items.length} running{group.folded ? ', folded' : ''}"
    title={project.path}
    onclick={onfold}
  >
    {#if project.pinned && group.items.length === 0}
      <span class="fold"><Icon name="pin" small /></span>
    {:else}
      <span class="fold"><Icon name={group.folded ? "chevron-right" : "chevron-down"} small /></span>
    {/if}
    <span class="nm">{project.name}</span>
    {#if group.folded}
      <span class="agg">
        <span class="pill mute">{group.items.length}</span>
        {#if needsYou > 0}
          <span class="pill need">! {needsYou} {needsYou === 1 ? "needs" : "need"} you</span>
        {:else if unread > 0}
          <span class="pill turn">• {unread} your turn</span>
        {/if}
      </span>
    {:else}
      <!-- The parent folder (the name already shows the last one), cut at the start; LRM keeps it left-to-right. -->
      <span class="path"
        >{project.pinned && group.items.length === 0
          ? "pinned"
          : `\u200e${tildePath(project.path.slice(0, Math.max(project.path.lastIndexOf("/"), 1)), home)}\u200e`}</span
      >
    {/if}
  </button>
  <button
    type="button"
    class="iconbtn"
    data-rove="plus:{project.id}"
    tabindex={roveKey === `plus:${project.id}` ? 0 : -1}
    aria-label="New or resume in {project.name}"
    title="New or resume in {project.name}"
    onclick={onnew}
  >
    <Icon name="plus" />
  </button>
</div>
{#if !group.folded}
  {#if project.missing}
    <div class="empty missing">Folder missing · {tildePath(project.path, home)}</div>
  {:else if group.items.length === 0}
    <div class="empty">Nothing running · + to start</div>
  {/if}
  {#each group.items as item (item.id)}
    {@const view = itemView(item, { tool: workspace.tool(item.tool), now: clock.now, home })}
    {@const dis = workspace.disambiguation.get(item.id) ?? ""}
    <SidebarRow
      {item}
      {view}
      {dis}
      label="{view.title}{dis}, {project.name}, {workspace.toolLabel(item.tool)}, {view.stateText}"
      selected={item.id === workspace.state?.activeItemId}
      {compact}
      rovable={roveKey === `row:${item.id}`}
      onselect={() => onselect(item)}
      onclose={() => onclose(item)}
    />
  {/each}
{/if}

<style>
  .proj {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 26px;
    padding: 0 2px 0 0;
    margin-top: 10px;
    flex: none;
    border-radius: 6px;
  }
  .proj:first-child {
    margin-top: 2px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0 2px 0 8px;
    border-radius: 6px;
    color: var(--dim);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
  .head:hover {
    background: var(--hover);
  }
  .fold {
    display: grid;
    place-items: center;
    width: 12px;
    height: 12px;
    color: #6d6d6d;
    flex: none;
    margin-left: -4px;
  }
  .nm {
    color: #a3a9ac;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .folded .nm {
    color: var(--text);
  }
  .path {
    text-transform: none;
    letter-spacing: 0;
    font-weight: 400;
    color: #6d6d6d;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    flex: 1;
    direction: rtl;
    text-align: left;
  }
  .agg {
    text-transform: none;
    letter-spacing: 0;
    font-weight: 500;
    display: flex;
    gap: 4px;
    margin-left: auto;
  }
  .empty {
    padding: 2px 10px 0 26px;
    font-size: 12px;
    line-height: 16px;
    color: #6d6d6d;
    flex: none;
  }
  .empty.missing {
    color: var(--yellow);
  }
</style>
