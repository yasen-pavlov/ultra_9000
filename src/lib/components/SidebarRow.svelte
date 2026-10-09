<script lang="ts">
  // One running item in the expanded sidebar. The whole row selects; the close button appears on
  // hover and selection. A failed launch attempt's Retry / Change… / Dismiss live in the panel.
  import type { ItemView } from "../items";
  import type { ItemState } from "../native";
  import Icon from "./Icon.svelte";
  import ToolTile from "./ToolTile.svelte";

  let {
    item,
    view,
    dis,
    label,
    selected,
    compact,
    rovable,
    onselect,
    onclose,
  }: {
    item: ItemState;
    view: ItemView;
    dis: string;
    label: string;
    selected: boolean;
    compact: boolean;
    /** Holds the sidebar's roving tab stop. */
    rovable: boolean;
    onselect: () => void;
    onclose: () => void;
  } = $props();
</script>

<div
  class="row {view.rowClass}"
  class:sel={selected}
  class:cmp={compact}
  data-item-id={item.id}
  data-need={item.activity === "needs-you" ? "" : undefined}
>
  <button
    type="button"
    class="sel-hit"
    data-rove="row:{item.id}"
    tabindex={rovable ? 0 : -1}
    aria-label={label}
    aria-current={selected ? "true" : undefined}
    onclick={onselect}
  ></button>
  <ToolTile
    tool={item.tool}
    model={item.model}
    badge={view.badge}
    badgeGlyph={view.badgeGlyph}
    unread={item.unread}
    size={compact ? 20 : 24}
  />
  <span class="tx">
    <span class="t"
      >{view.title}{#if dis !== ""}<span class="dis">{dis}</span>{/if}</span
    >
    {#if !compact}
      <!-- Board 21 (2026-10-04): line 2 is tool · model · age with the state pill at its end. -->
      <span class="sl">
        <span class="s" class:err={view.subIsError} title={view.sub}>{view.sub}</span>
        {#if view.pill !== ""}<span class="pill {view.pillClass}">{view.pill}</span>{/if}
      </span>
    {/if}
  </span>
  {#if compact && view.pill !== ""}
    <span class="pill {view.pillClass}">{view.pill}</span>
  {/if}
  <button type="button" class="iconbtn x" tabindex="-1" aria-label="Close {view.title}" title="Close" onclick={onclose}>
    <Icon name="close" small />
  </button>
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 9px;
    min-height: 42px;
    padding: 4px 6px 4px 8px;
    border-radius: 8px;
    position: relative;
    flex: none;
  }
  .row.cmp {
    min-height: 30px;
  }
  /* --row-bg feeds the tile's provider plate and badge rings; selection wins over hover. */
  .row:hover {
    --row-bg: var(--hover);
    background: var(--row-bg);
  }
  .row.sel {
    --row-bg: var(--sel);
    background: var(--row-bg);
  }
  .row:has(.sel-hit:focus-visible) {
    box-shadow: inset 0 0 0 2px var(--blue);
  }
  .sel-hit {
    position: absolute;
    inset: 0;
    border: 0;
    background: transparent;
    border-radius: 8px;
  }
  .sel-hit:focus-visible {
    outline: none;
  }
  .tx {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
    gap: 0;
    pointer-events: none;
  }
  .t {
    color: var(--hi);
    font-size: 13px;
    line-height: 18px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dis {
    color: var(--dim);
    font-weight: 400;
  }
  .sl {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    height: 16px;
  }
  .s {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    line-height: 16px;
    color: var(--dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .s.err {
    color: var(--err-text);
  }
  .pill {
    pointer-events: none;
    flex: none;
  }
  .sl .pill {
    line-height: 14px;
  }
  /* The close button floats over the title's end (18 px reserved only while it shows), so the pill
     and sub line never shift; compact rows reserve it at the row's right instead. */
  .x {
    position: absolute;
    top: 4px;
    right: 4px;
    width: 18px;
    height: 18px;
    z-index: 1;
    opacity: 0;
    pointer-events: none;
  }
  .row:hover .x,
  .row.sel .x,
  .row:focus-within .x {
    opacity: 1;
    pointer-events: auto;
  }
  .row:hover .t,
  .row.sel .t,
  .row:focus-within .t {
    padding-right: 18px;
  }
  .row.cmp .x {
    top: 6px;
  }
  .row.cmp:hover,
  .row.cmp.sel,
  .row.cmp:focus-within {
    padding-right: 30px;
  }
  .row.cmp:hover .t,
  .row.cmp.sel .t,
  .row.cmp:focus-within .t {
    padding-right: 0;
  }
  .row.exited .t,
  .row.pending .t {
    color: var(--text);
  }
  /* Dim the tool mark only, never the state badge or unread dot (board 22): opacity on the
     whole tile would undo the badge's contrast. */
  .row.exited :global(.tile > :not(.bd, .unread)) {
    opacity: 0.55;
  }
  .row.pending :global(.tile > :not(.bd, .unread)) {
    opacity: 0.6;
  }
</style>
