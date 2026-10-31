<script lang="ts">
  // One tool tile (boards 23–26): the tool's mark at its exact box size and an integer offset on
  // the shared tile, with the provider plate (top-left), unread dot (top-right) and state badge
  // (bottom-right). Size 16 is the bare mark without a tile (context bar, launch picker).
  // `icons.brand_color` decides where colour shows (board 24): "all" (default) everywhere; "pickers"
  // only where `colour` marks a pick surface (saved conversations in Open, the tool menu); "launch"
  // only the tool menu; "off" nowhere. Coloured, brand artwork replaces the mono mark and a tool
  // accent colours single-colour marks; the tile itself stays neutral (board 24, option D).
  import type { Badge } from "../items";
  import { MARK_BOX, showsProvider, toolMark, type TileSize } from "../icons";
  import { settings } from "../settings.svelte";
  import { workspace } from "../workspace.svelte";
  import ProviderMark from "./ProviderMark.svelte";

  let {
    tool,
    model = null,
    size = 24,
    colour,
    badge = "",
    badgeGlyph = "",
    unread = false,
  }: {
    tool: string;
    /** The agent's current model; its provider goes on the corner plate (24 and 30 px tiles only). */
    model?: string | null;
    size?: TileSize;
    /** A surface where colour is allowed; omitted = monochrome. */
    colour?: "launch" | "picker";
    badge?: Badge;
    badgeGlyph?: string;
    unread?: boolean;
  } = $props();

  const box = $derived(MARK_BOX[size]);
  const info = $derived(workspace.tool(tool));
  const coloured = $derived(
    settings.brandColour === "all" ||
      (colour === "launch"
        ? settings.brandColour !== "off"
        : colour === "picker" && settings.brandColour === "pickers"),
  );
  const mark = $derived(toolMark(tool, info, box, coloured));
  /**
   * Mark colour: the tool's accent, or neutral grey for the generic agent mark (board 24) and the
   * document mark (board 30, every brand-colour mode except "off"); null = default.
   */
  const markColour = $derived(
    mark.kind === "svg" && mark.neutral === true
      ? settings.brandColour === "off"
        ? null
        : "#b0bec5"
      : coloured
        ? (info?.accent ?? (mark.kind === "svg" && mark.generic === true ? "#b0bec5" : null))
        : null,
  );
  /** icon_fill: the square logo is the tile surface (bare marks have no tile, so they contain it). Independent of the tint. */
  const fill = $derived(mark.kind === "user" && mark.fill && size !== 16);
  const plate = $derived(
    model !== null && size >= 24 && settings.providerMark === "corner" && showsProvider(tool, model) ? model : null,
  );
</script>

<!-- eslint-disable svelte/no-at-html-tags -- tool marks are bundled build-time SVGs (icons.ts), never user input -->
<span class="tile s{size}" class:bare={size === 16} style:--tile-mark={markColour} aria-hidden="true"
  >{#if mark.kind === "svg"}<span class="m">{@html mark.svg}</span>{:else if mark.kind === "img"}<img
      class="m"
      src={mark.url}
      width={box}
      height={box}
      alt=""
      draggable="false"
    />{:else if mark.kind === "glyph"}<span class="g">{mark.text}</span
    >{:else if mark.tint === "original"}{#if fill}<img
        class="fill"
        src={mark.url}
        width={size}
        height={size}
        alt=""
        draggable="false"
      /><span class="edge"></span>{:else}<img
        class="m contain"
        src={mark.url}
        width={box}
        height={box}
        alt=""
        draggable="false"
      />{/if}{:else if fill}<span class="fill mask" style:--icon={`url("${mark.url}")`}></span><span class="edge"
    ></span>{:else}<span class="m mask" style:--icon={`url("${mark.url}")`}></span>{/if}{#if plate !== null}<span
      class="pv"><ProviderMark model={plate} {tool} /></span
    >{/if}{#if badge !== ""}<span class="bd {badge}">{badgeGlyph}</span>{/if}{#if unread}<span class="unread"
    ></span>{/if}</span
>
<!-- eslint-enable svelte/no-at-html-tags -->
