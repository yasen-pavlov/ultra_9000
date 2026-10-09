<script lang="ts">
  // The one blank rectangle reserved for the native Alacritty view. Nothing is drawn here by the
  // web layer; native positions the selected item's view above it from the reported geometry. It
  // stays mounted for the app's lifetime and only toggles visibility, so hiding never races a
  // remount. Sheets, the launcher and launch-attempt panels replace it rather than overlay it.
  import { untrack } from "svelte";
  import { trackViewport, type ViewportTarget, type ViewportTracker } from "../viewport";

  let { itemId, presentationEpoch, visible }: ViewportTarget = $props();

  let host: HTMLDivElement;
  let tracker: ViewportTracker | null = null;

  $effect(() => {
    const current = trackViewport(
      host,
      untrack(() => ({ itemId, presentationEpoch, visible })),
    );
    tracker = current;
    return () => {
      current.dispose();
      if (tracker === current) tracker = null;
    };
  });

  $effect(() => {
    tracker?.setTarget({ itemId, presentationEpoch, visible });
  });
</script>

<div class="terminal-host" bind:this={host} hidden={!visible} role="region" aria-label="Terminal"></div>

<style>
  /* Fills the content panel's content box edge to edge. Native reads the panel's left corner radii
     (where the host sits flush in those corners) from the viewport report and masks the native
     view's corners to match, so the terminal grid starts at the panel edge like standalone Alacritty. */
  .terminal-host {
    flex: 1 1 auto;
    min-width: 0;
    min-height: 0;
    background: var(--panel);
  }
</style>
