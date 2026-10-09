<script lang="ts">
  // Close confirmation (board 13): only for a working/needs-you OMP agent, a shell with a foreground
  // job, Neovim, or (opt-in) an unmonitored agent. Default focus is the safe button; Esc cancels;
  // pressing the close shortcut again confirms natively.
  import { clock } from "../clock.svelte";
  import { run } from "../errors.svelte";
  import { basename, modelShort, spokenAge } from "../format";
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
  // The executable label is stable even when zsh sets the terminal title to the job command.
  const shellLabel = $derived(item === null ? "The shell" : basename(workspace.tool(item.tool)?.path ?? "") || label);
  const project = $derived(item === null ? "" : workspace.projectName(item.projectId));
  const closeLabel = $derived(workspace.shortcut("close"));
  const endLabel = $derived(
    reason === "foreground-job" ? "End shell" : reason === "neovim" ? "End Neovim" : "End agent",
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
        <h2 id="confirm-title">End “{item?.title ?? "this item"}”?</h2>
        {#if meta !== ""}<div class="meta">{meta}</div>{/if}
      </div>
    </div>
    <div id="confirm-body" class="body">
      {#if reason === "agent-working"}
        <p>
          {label} is in the middle of a turn. Ending now stops it:
          <span class="strong">this turn's unfinished work is lost</span>. Changes it already wrote to files stay
          written.
        </p>
        <p class="dim">
          The conversation up to the last completed turn stays saved in {project}, so you can resume it.
        </p>
      {:else if reason === "agent-needs-you"}
        <p>
          {label} is waiting for your {item?.attention === "question"
            ? "answer to a question"
            : item?.attention === "approval"
              ? "approval"
              : "reply"}. Ending now drops that pending turn: <span class="strong">its unfinished work is lost</span>.
          Changes it already wrote to files stay written.
        </p>
        <p class="dim">
          The conversation up to the last completed turn stays saved in {project}, so you can resume it.
        </p>
      {:else if reason === "foreground-job"}
        <p>
          {shellLabel} is running <span class="strong">{detail ?? "a foreground job"}</span>. Ending the shell ends that
          job too, and the screen and scrollback are gone. <span class="strong">This can't be undone.</span>
        </p>
      {:else if reason === "neovim"}
        <p>
          Neovim may hold unsaved buffers that Ultra 9000 cannot see. Ending it sends a hangup: <span class="strong"
            >unsaved changes may be lost</span
          >.
        </p>
        <p class="dim">Neovim's own swap files may help recover them.</p>
      {:else}
        <p>
          {label} reports no turn state, so Ultra 9000 can't tell whether it is in the middle of something. Ending it stops
          the process.
        </p>
        <p class="dim">{label} keeps its own saved conversation; resume it from the palette.</p>
      {/if}
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
        {endLabel}{#if closeLabel !== null}<kbd>{closeLabel}</kbd>{/if}
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
