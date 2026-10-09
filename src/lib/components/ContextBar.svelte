<script lang="ts">
  // Names the selected item when the sidebar can't (collapsed, or layout.context_bar = "always").
  // Hovering a collapsed-strip icon previews that item here in blue, because HTML tooltips cannot
  // cover the native terminal. While the sidebar is collapsed it is also the outlet for notices
  // and command errors, beside the item or the open sheet's name. Its empty space is the window
  // drag region.
  import { clock } from "../clock.svelte";
  import { run, uiErrors } from "../errors.svelte";
  import { modelShort } from "../format";
  import { itemView } from "../items";
  import { itemClose, itemResume, noticeDismiss, type ShortcutAction } from "../native";
  import { workspace } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import ProviderMark from "./ProviderMark.svelte";
  import ToolTile from "./ToolTile.svelte";

  let {
    peekId,
    compact,
    messages,
    viewportError,
  }: {
    peekId: string | null;
    compact: boolean;
    /** The sidebar can't show notices and errors (collapsed): they appear here instead, sheet open or not. */
    messages: boolean;
    viewportError: string | null;
  } = $props();

  const home = $derived(workspace.state?.defaultCwd ?? "");
  const active = $derived(workspace.active);
  const sheet = $derived(workspace.state?.sheet ?? null);
  const peek = $derived(peekId === null || peekId === active?.id ? null : (workspace.itemsById.get(peekId) ?? null));
  const peekView = $derived(
    peek === null ? null : itemView(peek, { tool: workspace.tool(peek.tool), now: clock.now, home }),
  );
  const peekShortcut = $derived.by(() => {
    if (peek === null) return null;
    const position = workspace.visibleOrder.findIndex((item) => item.id === peek.id);
    return position >= 0 && position < 9 ? workspace.shortcut(`item-${position + 1}` as ShortcutAction) : null;
  });
  const activeView = $derived(
    active === null ? null : itemView(active, { tool: workspace.tool(active.tool), now: clock.now, home }),
  );
  const sheetTitle = $derived.by(() => {
    switch (sheet?.kind) {
      case "palette":
        return {
          name: "Open",
          detail: sheet.projectId === null ? "" : `searching ${workspace.projectName(sheet.projectId)}`,
        };
      case "settings":
        return { name: "Settings", detail: "" };
      case "shortcuts":
        return { name: "Keyboard shortcuts", detail: "" };
      case "confirm-close":
        return { name: "Close", detail: "confirm" };
      case "quit":
        return { name: "Quit", detail: "confirm" };
      default:
        return null;
    }
  });
  /** The newest native notice; an error notice (launch/action failure) is styled and announced as one. */
  const notice = $derived((workspace.state?.notices ?? []).at(-1) ?? null);
  const closeLabel = $derived(workspace.shortcut("close"));
  /** The peek's secondary line split around the model's short name, so its provider logo sits before it. */
  const peekSub = $derived.by(() => {
    if (peekView === null || peekView.subModel === null) return null;
    const short = modelShort(peekView.subModel);
    const at = peekView.sub.indexOf(short, Math.max(0, peekView.sub.indexOf(" · ")));
    return at < 0
      ? null
      : { before: peekView.sub.slice(0, at), model: peekView.subModel, rest: peekView.sub.slice(at) };
  });
</script>

{#snippet message()}
  {#if uiErrors.list.length > 0}
    {@const last = uiErrors.list[uiErrors.list.length - 1]}
    <span class="txt cerr" role="alert" title={last.message}>{last.message}</span>
    <button type="button" class="ctxbtn" aria-label="Dismiss error" onclick={() => uiErrors.dismiss(last.id)}>
      <Icon name="close" small />
    </button>
  {:else if viewportError !== null}
    <span class="txt cerr" role="alert" title={viewportError}>{viewportError}</span>
  {:else if notice !== null}
    {@const error = notice.kind === "error"}
    <span
      class="txt"
      class:notice={!error}
      class:cerr={error}
      role={error ? "alert" : "status"}
      title={notice.body === "" ? notice.title : `${notice.title}: ${notice.body}`}>{notice.title}</span
    >
    {#if notice.conversation !== null}
      {@const conversation = notice.conversation}
      <button
        type="button"
        class="ctxbtn"
        onclick={() => run("Resume", () => itemResume(conversation.tool, conversation.id, conversation.path))}
        >Resume</button
      >
    {/if}
    <button
      type="button"
      class="ctxbtn"
      aria-label={error ? "Dismiss error" : "Dismiss notice"}
      onclick={() => run("Dismiss", () => noticeDismiss(notice.id))}
    >
      <Icon name="close" small />
    </button>
  {/if}
{/snippet}

<div class="ctx" class:peek={peek !== null} class:compact data-tauri-drag-region>
  {#if peek !== null && peekView !== null}
    <span class="txt"><Icon name="arrow-right" small /></span>
    <span class="txt pproj">{workspace.projectName(peek.projectId)}</span><span class="txt sep">/</span>
    <b class="txt ptitle">{peekView.title}{workspace.disambiguation.get(peek.id) ?? ""}</b>
    {#if peekView.pill !== ""}<span class="pill {peekView.pillClass}">{peekView.pill}</span>{/if}
    <span class="txt psub"
      >{#if peekSub !== null}{peekSub.before}<ProviderMark
          model={peekSub.model}
          tool={peek.tool}
          inline
        />{peekSub.rest}{:else}{peekView.sub}{/if} · click{peekShortcut === null ? "" : ` or ${peekShortcut}`}</span
    >
  {:else if sheetTitle !== null}
    <b class="txt">{sheetTitle.name}</b>
    {#if sheetTitle.detail !== ""}<span class="txt sep">·</span><span class="txt">{sheetTitle.detail}</span>{/if}
    {#if messages}{@render message()}{/if}
    {#if active !== null}
      <span class="r txt">Esc returns to “{active.title}”</span>
    {/if}
  {:else}
    {#if active !== null && activeView !== null}
      <ToolTile tool={active.tool} size={16} />
      <b class="txt">{workspace.projectName(active.projectId)}</b><span class="txt sep">/</span>
      <b class="txt title">{activeView.title}{workspace.disambiguation.get(active.id) ?? ""}</b>
      <span class="pill {activeView.pillClass === '' ? 'mute' : activeView.pillClass}"
        >{activeView.pillClass === "job" ? activeView.pill : activeView.stateText}</span
      >
      {#if active.model !== null}
        <span class="txt model"><ProviderMark model={active.model} tool={active.tool} inline />{active.model}</span>
      {/if}
    {/if}
    {#if messages}{@render message()}{/if}
    {#if active !== null}
      <span class="r">
        <button type="button" class="ctxbtn" onclick={() => run("Close", () => itemClose(active.id, false))}>
          Close{#if closeLabel !== null}<kbd>{closeLabel}</kbd>{/if}
        </button>
      </span>
    {/if}
  {/if}
</div>

<style>
  .ctx {
    height: 30px;
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 6px;
    font-size: 12.5px;
    color: var(--dim);
    white-space: nowrap;
    overflow: hidden;
  }
  .ctx.compact {
    height: 26px;
    font-size: 12px;
  }
  .txt {
    pointer-events: none;
    display: inline-flex;
    align-items: center;
  }
  b {
    color: var(--hi);
    font-weight: 600;
  }
  .title,
  .ptitle {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    display: inline-block;
  }
  .sep {
    color: #555;
  }
  .r {
    margin-left: auto;
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .ctx.peek {
    color: var(--blue);
  }
  .ctx.peek .ptitle {
    color: #cfe0ff;
  }
  .psub {
    color: #7d93bd;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    display: inline-block;
  }
  .notice,
  .cerr {
    margin-left: 12px;
  }
  .cerr {
    color: var(--err-text);
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    display: inline-block;
  }
  .ctxbtn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 6px;
    border-radius: 6px;
    color: var(--dim);
    font-size: 12px;
  }
  .ctxbtn:hover {
    background: var(--hover);
    color: var(--hi);
  }
</style>
