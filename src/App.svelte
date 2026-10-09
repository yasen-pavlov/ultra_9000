<script lang="ts">
  // Window layout: sidebar | (context bar) + content panel. The panel shows exactly one thing: the
  // selected item's native terminal (with an exit bar when it ended), a launch attempt, a sheet
  // (palette, settings, shortcuts, close confirmation, quit) that replaces the terminal, or the
  // cold-start launcher. Native owns all state; this only renders snapshots and issues commands.
  import { clock } from "./lib/clock.svelte";
  import ConfirmClose from "./lib/components/ConfirmClose.svelte";
  import ContextBar from "./lib/components/ContextBar.svelte";
  import ExitBar from "./lib/components/ExitBar.svelte";
  import LaunchAttempt from "./lib/components/LaunchAttempt.svelte";
  import Palette from "./lib/components/Palette.svelte";
  import QuitSheet from "./lib/components/QuitSheet.svelte";
  import SettingsSheet from "./lib/components/SettingsSheet.svelte";
  import ShortcutsSheet from "./lib/components/ShortcutsSheet.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import TerminalHost from "./lib/components/TerminalHost.svelte";
  import { run, uiErrors } from "./lib/errors.svelte";
  import { errorMessage, listenUiCommand, listenWindowFocus, uiSet, type UnlistenFn } from "./lib/native";
  import { settings } from "./lib/settings.svelte";
  import { cancelTerminalFocus, onViewportError, requestTerminalFocus } from "./lib/viewport";
  import { workspace } from "./lib/workspace.svelte";

  let viewportError = $state<string | null>(null);
  let peekId = $state<string | null>(null);
  let windowWidth = $state(window.innerWidth);
  let windowHeight = $state(window.innerHeight);
  /** Expanded by hand while the window is narrower than sidebar.auto_collapse_below. */
  let expandedWhileNarrow = $state(false);
  let sidebar = $state<ReturnType<typeof Sidebar> | null>(null);

  // Native items outlive this page; the app shell only observes them for its lifetime.
  $effect(() => workspace.connect());
  $effect(() => settings.connect());
  $effect(() => clock.start());
  $effect(() =>
    onViewportError((error) => {
      viewportError = error;
    }),
  );

  /** Subscribes for this component's lifetime, tolerating teardown before the listener resolves. */
  function subscribe(label: string, listen: () => Promise<UnlistenFn>): () => void {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    listen().then(
      (stop) => {
        if (disposed) stop();
        else unlisten = stop;
      },
      (error) => {
        if (!disposed) viewportError = `Cannot observe ${label}: ${errorMessage(error)}`;
      },
    );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }

  // Hiding/minimizing drops queued focus; the window becoming key again restores terminal focus
  // from the current geometry, unless the user had moved keyboard focus into the sidebar.
  $effect(() =>
    subscribe("window focus", () =>
      listenWindowFocus((focused) => {
        if (!focused) cancelTerminalFocus();
        else if (document.activeElement?.closest("[data-sidebar]") == null) requestTerminalFocus();
      }),
    ),
  );

  // Native moved keyboard focus from the terminal to the web view first (focus-sidebar).
  $effect(() =>
    subscribe("UI commands", () =>
      listenUiCommand((command) => {
        if (command.command === "focus-sidebar") sidebar?.focusList();
      }),
    ),
  );

  const snapshot = $derived(workspace.state);
  const sheet = $derived(snapshot?.sheet ?? null);
  const active = $derived(workspace.active);
  const preferExpanded = $derived(snapshot?.ui.sidebarExpanded ?? true);
  const narrow = $derived(windowWidth < settings.autoCollapseBelow);
  const expanded = $derived(preferExpanded && (!narrow || expandedWhileNarrow));
  // Collapsed, the context bar is the only outlet for notices and errors (NoticeList lives in the
  // expanded sidebar), so layout.context_bar = "never" still shows it while one is open.
  const hasMessages = $derived(
    uiErrors.list.length > 0 || viewportError !== null || (snapshot?.notices.length ?? 0) > 0,
  );
  const showContext = $derived(
    settings.contextBar === "always" || (!expanded && (settings.contextBar === "auto" || hasMessages)),
  );

  $effect(() => {
    if (!narrow) expandedWhileNarrow = false;
  });

  function toggleSidebar(): void {
    if (narrow && preferExpanded) {
      expandedWhileNarrow = !expandedWhileNarrow;
      return;
    }
    if (narrow) expandedWhileNarrow = true;
    void run("Sidebar", () => uiSet({ sidebarExpanded: !preferExpanded }));
  }

  const panelLabel = $derived.by(() => {
    switch (sheet?.kind) {
      case "palette":
        return "Open";
      case "settings":
        return "Settings";
      case "shortcuts":
        return "Keyboard shortcuts";
      case "confirm-close":
        return "Close confirmation";
      case "quit":
        return "Quit confirmation";
      default:
        return active === null ? "Start or resume" : `Terminal: ${active.title}`;
    }
  });
</script>

<svelte:window bind:innerWidth={windowWidth} bind:innerHeight={windowHeight} />

<div class="win">
  {#if snapshot !== null}
    <Sidebar bind:this={sidebar} {expanded} {viewportError} ontoggle={toggleSidebar} onpeek={(id) => (peekId = id)} />
  {/if}
  <main class="main">
    {#if snapshot !== null && showContext}
      <ContextBar {peekId} {viewportError} compact={windowHeight < 480} messages={!expanded} />
    {/if}
    <section class="panel" aria-label={panelLabel}>
      {#if sheet === null && active !== null && active.process === "exited"}
        <ExitBar item={active} />
      {/if}
      <TerminalHost
        itemId={snapshot?.activeItemId ?? null}
        presentationEpoch={snapshot?.presentationEpoch ?? 0}
        visible={workspace.terminalVisible}
      />
      {#if snapshot === null}
        <div class="connecting">
          {#if workspace.error !== null}
            <p class="error selectable" role="alert">{workspace.error}</p>
            <button type="button" class="btn" onclick={() => workspace.retry()}>Retry</button>
          {:else}
            <p>Connecting…</p>
          {/if}
        </div>
      {:else if sheet !== null}
        {#if sheet.kind === "palette"}
          {#key `${snapshot.presentationEpoch}|${sheet.projectId ?? ""}|${sheet.mode}`}
            <Palette variant="sheet" initialScope={sheet.projectId} mode={sheet.mode} />
          {/key}
        {:else if sheet.kind === "settings"}
          <SettingsSheet sidebarExpanded={preferExpanded} />
        {:else if sheet.kind === "shortcuts"}
          <ShortcutsSheet />
        {:else if sheet.kind === "confirm-close"}
          {#key sheet.itemId}
            <ConfirmClose itemId={sheet.itemId} reason={sheet.reason} detail={sheet.detail} />
          {/key}
        {:else if sheet.kind === "quit"}
          <QuitSheet itemIds={sheet.itemIds} />
        {/if}
      {:else if active === null}
        {#key workspace.lastProjectId}
          <Palette variant="launcher" initialScope={workspace.lastProjectId} mode="open" />
        {/key}
      {:else if active.process === "starting" || active.process === "failed"}
        <LaunchAttempt item={active} />
      {/if}
    </section>
  </main>
</div>

<style>
  .win {
    width: 100%;
    height: 100%;
    display: flex;
    background: var(--chrome);
    color: var(--text);
    overflow: hidden;
  }
  /* Edge-to-edge: the content column runs flush to the window's top, right and bottom edges. */
  .main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    padding: 0;
  }
  /* Rounded on the sidebar side only, no border: the chrome/panel colour step marks the inset. The
     native terminal fills the panel and is corner-masked natively to these radii (read from here).
     Right corners stay square; the window manager rounds the window itself. */
  .panel {
    flex: 1;
    background: var(--panel);
    border-radius: 10px 0 0 10px;
    overflow: hidden;
    position: relative;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  /* Small tiles (boards 07 and 08, 640×400): radius 8. */
  @media (max-width: 760px), (max-height: 520px) {
    .panel {
      border-radius: 8px 0 0 8px;
    }
  }
  .connecting {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: var(--dim);
  }
  .connecting p {
    margin: 0;
  }
  .connecting .error {
    color: var(--err-text);
  }
</style>
