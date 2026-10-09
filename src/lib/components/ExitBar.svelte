<script lang="ts">
  // Above an exited item's final screen: what ended, and Resume / Start again / Close.
  import { run } from "../errors.svelte";
  import { clockTime } from "../format";
  import { itemClose, itemRestart, type ItemState } from "../native";
  import { workspace } from "../workspace.svelte";
  import Icon from "./Icon.svelte";

  let { item }: { item: ItemState } = $props();

  const label = $derived(workspace.toolLabel(item.tool));
  const agent = $derived(item.kind === "agent");
  const clean = $derived(!agent && item.exitCode === 0);
  const closeLabel = $derived(workspace.shortcut("close"));
  const details = $derived(
    [
      `at ${clockTime(item.stateSince)}`,
      item.exitCode === null ? (agent ? null : "ended by a signal") : `exit code ${item.exitCode}`,
      "final screen below",
      agent && item.conversation !== null ? "the conversation is saved" : null,
    ]
      .filter((part) => part !== null)
      .join(" · "),
  );
</script>

<div class="exitbar" class:ok={clean} role="status">
  <Icon name={agent ? "error" : "panel"} color={agent ? "#ff5370" : clean ? "#c3e88d" : "#8c8b8b"} />
  <b>{agent ? `${label} stopped with an error` : `${item.title} exited`}</b>
  <span class="grow">{details}</span>
  <button
    type="button"
    class="btn sm pri"
    onclick={() => run(agent && item.conversation !== null ? "Resume" : "Start again", () => itemRestart(item.id))}
  >
    {agent && item.conversation !== null ? "Resume" : "Start again"}
  </button>
  <button type="button" class="btn sm" onclick={() => run("Close", () => itemClose(item.id, false))}>
    Close{#if closeLabel !== null}<kbd>{closeLabel}</kbd>{/if}
  </button>
</div>

<style>
  .exitbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px 8px 14px;
    border-bottom: 1px solid var(--line);
    background: #241c1e;
    font-size: 12.5px;
    color: var(--hi);
    white-space: nowrap;
  }
  .exitbar.ok {
    background: #1f2124;
  }
  b {
    font-weight: 600;
  }
  .grow {
    flex: 1;
    min-width: 0;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
