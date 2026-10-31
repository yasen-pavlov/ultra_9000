<script lang="ts">
  // A project header (folds on click; keeps an aggregate while folded) and its running items.
  import { clock } from "../clock.svelte";
  import { tildePath } from "../format";
  import { itemView, rowLabel } from "../items";
  import type { ItemState } from "../native";
  import { settings } from "../settings.svelte";
  import { workspace, type ProjectGroup } from "../workspace.svelte";
  import ProjectHead from "./ProjectHead.svelte";
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
  const compact = $derived(settings.density === "compact");
</script>

<ProjectHead {project} items={group.items} folded={group.folded} {roveKey} {onfold} {onnew} />
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
      label={rowLabel(view, dis, project.name, workspace.toolLabel(item.tool))}
      selected={item.id === workspace.state?.activeItemId}
      {compact}
      rovable={roveKey === `row:${item.id}`}
      onselect={() => onselect(item)}
      onclose={() => onclose(item)}
    />
  {/each}
{/if}

<style>
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
