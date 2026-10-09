<script lang="ts">
  // The model provider's 10 px logo, always mono (#b0bec5): on a tile's corner plate, or inline
  // before a model name. Never drawn smaller than 10 px (board 25). Nothing is drawn when the tool
  // already implies the maker (Claude Code on Anthropic, Codex on OpenAI).
  import { providerMark, showsProvider } from "../icons";
  import { workspace } from "../workspace.svelte";

  let { model, tool, inline = false }: { model: string; tool: string; inline?: boolean } = $props();

  const mark = $derived(showsProvider(tool, model) ? providerMark(model, workspace.state?.providerIcons ?? {}) : null);
</script>

{#if mark?.kind === "svg"}
  <!-- eslint-disable-next-line svelte/no-at-html-tags -- bundled build-time SVG (icons.ts), never user input -->
  <span class="pm" class:inline aria-hidden="true">{@html mark.svg}</span>
{:else if mark?.kind === "user"}
  <span class="pm mask" class:inline style:--icon={`url("${mark.url}")`} aria-hidden="true"></span>
{/if}

<style>
  .pm {
    display: block;
    width: 10px;
    height: 10px;
    color: #b0bec5;
    flex: none;
  }
  .pm :global(svg) {
    display: block;
  }
  .pm.inline {
    display: inline-block;
    margin-right: 4px;
    vertical-align: -1px;
  }
  .mask {
    background-color: currentColor;
    -webkit-mask: var(--icon) center / contain no-repeat;
    mask: var(--icon) center / contain no-repeat;
  }
</style>
