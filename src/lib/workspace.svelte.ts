// Mirrors the native WorkspaceState. Native owns items, projects, selection, sheets and order; this
// store only observes snapshots (strictly newer revisions) and derives presentation from them.
import { clock } from "./clock.svelte";
import { basename, clockTime } from "./format";
import {
  errorMessage,
  fetchWorkspaceState,
  listenWorkspaceState,
  type ItemState,
  type ProjectState,
  type ShortcutAction,
  type ToolInfo,
  type WorkspaceState,
} from "./native";

export interface ProjectGroup {
  project: ProjectState;
  items: ItemState[];
  folded: boolean;
}

/** Ordering for "what's waiting": needs-you first, then unread your-turn, oldest state first. */
function waitingOrder(items: readonly ItemState[]): ItemState[] {
  const needsYou = items.filter((item) => item.activity === "needs-you").sort((a, b) => a.stateSince - b.stateSince);
  const unread = items
    .filter((item) => item.activity === "your-turn" && item.unread)
    .sort((a, b) => a.stateSince - b.stateSince);
  return [...needsYou, ...unread];
}

class WorkspaceStore {
  state = $state.raw<WorkspaceState | null>(null);
  /** Failure to reach or read native workspace state (not an item exit or launch failure). */
  error = $state<string | null>(null);
  /** Project of the most recently active item; scopes the launcher once nothing is selected. */
  lastProjectId = $state<string | null>(null);

  itemsById: ReadonlyMap<string, ItemState> = $derived(
    new Map((this.state?.items ?? []).map((item) => [item.id, item])),
  );
  projectsById: ReadonlyMap<string, ProjectState> = $derived(
    new Map((this.state?.projects ?? []).map((project) => [project.id, project])),
  );
  toolsById: ReadonlyMap<string, ToolInfo> = $derived(
    new Map((this.state?.tools ?? []).map((tool) => [tool.id, tool])),
  );

  active: ItemState | null = $derived(
    this.state?.activeItemId == null ? null : (this.itemsById.get(this.state.activeItemId) ?? null),
  );

  /** The native surface exists and the terminal view presents it. */
  terminalVisible: boolean = $derived(
    this.state?.view === "terminal" &&
      this.active !== null &&
      (this.active.process === "running" || this.active.process === "exited"),
  );

  /** Sidebar groups in native order; items in each project's launch order. */
  groups: ProjectGroup[] = $derived.by(() => {
    const state = this.state;
    if (state === null) return [];
    const folded = new Set(state.ui.foldedProjectIds);
    return state.projects.map((project) => ({
      project,
      items: project.itemIds.flatMap((id) => {
        const item = this.itemsById.get(id);
        return item === undefined ? [] : [item];
      }),
      folded: folded.has(project.id),
    }));
  });

  /** Items reachable by item 1…9: sidebar order, folded groups skipped. */
  visibleOrder: ItemState[] = $derived(this.groups.flatMap((group) => (group.folded ? [] : group.items)));

  waiting: ItemState[] = $derived(waitingOrder(this.state?.items ?? []));
  needsYouCount: number = $derived(this.waiting.filter((item) => item.activity === "needs-you").length);
  unreadCount: number = $derived(this.waiting.length - this.needsYouCount);

  /** The MRU item before the active one ("previous item"), if still present. */
  previousItemId: string | null = $derived(
    (this.state?.recentItemIds ?? []).find((id) => id !== this.state?.activeItemId && this.itemsById.has(id)) ?? null,
  );

  /** Start-time suffix (" · 14:02") for items sharing a title within one project. */
  disambiguation: ReadonlyMap<string, string> = $derived.by(() => {
    const suffixes = new Map<string, string>();
    for (const group of this.groups) {
      const byTitle = new Map<string, ItemState[]>();
      for (const item of group.items) byTitle.set(item.title, [...(byTitle.get(item.title) ?? []), item]);
      for (const same of byTitle.values()) {
        if (same.length < 2) continue;
        for (const item of same) suffixes.set(item.id, ` · ${clockTime(item.startedAt)}`);
      }
    }
    return suffixes;
  });

  readonly #shortcutLabels: ReadonlyMap<ShortcutAction, string | null> = $derived(
    new Map((this.state?.shortcuts ?? []).map((shortcut) => [shortcut.action, shortcut.label])),
  );

  #disconnect: (() => void) | null = null;
  #liveTimer: number | undefined;

  /** Platform label of an app shortcut ("Ctrl+Shift+Space"), or null when unbound. */
  shortcut(action: ShortcutAction): string | null {
    return this.#shortcutLabels.get(action) ?? null;
  }

  tool(id: string): ToolInfo | undefined {
    return this.toolsById.get(id);
  }

  toolLabel(id: string): string {
    return this.toolsById.get(id)?.label ?? id;
  }

  /** Project display name; projects without running items fall back to the folder name. */
  projectName(projectId: string | null): string {
    if (projectId === null) return "";
    const known = this.projectsById.get(projectId);
    if (known !== undefined) return known.name;
    return basename(projectId);
  }

  /** Applies only strictly newer native revisions; older fetches or events are stale. */
  #apply(next: WorkspaceState): void {
    if (this.state !== null && next.revision <= this.state.revision) return;
    this.state = next;
    this.error = null;
    const active = next.activeItemId === null ? undefined : next.items.find((item) => item.id === next.activeItemId);
    if (active !== undefined) this.lastProjectId = active.projectId;
    this.#scheduleLiveEnd(next);
  }

  /**
   * A document row reads “live” until `liveUntilMs` (board 30). The coarse clock would leave it up
   * to a tick late, so the clock is refreshed at the earliest pending end.
   */
  #scheduleLiveEnd(state: WorkspaceState): void {
    clearTimeout(this.#liveTimer);
    const now = Date.now();
    const ends = state.items.flatMap((item) =>
      item.document !== null && item.document.liveUntilMs > now ? [item.document.liveUntilMs] : [],
    );
    if (ends.length === 0) return;
    this.#liveTimer = window.setTimeout(
      () => {
        clock.now = Date.now();
        if (this.state !== null) this.#scheduleLiveEnd(this.state);
      },
      Math.min(...ends) - now + 20,
    );
  }

  /** Subscribe first, then fetch, so no transition between the two is lost. Returns cleanup. */
  connect(): () => void {
    this.#disconnect?.();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const disconnect = () => {
      disposed = true;
      unlisten?.();
      unlisten = null;
      if (this.#disconnect === disconnect) this.#disconnect = null;
    };
    this.#disconnect = disconnect;

    void (async () => {
      try {
        const stop = await listenWorkspaceState((next) => {
          if (!disposed) this.#apply(next);
        });
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
      } catch (error) {
        if (!disposed) this.error = `Cannot subscribe to workspace state: ${errorMessage(error)}`;
        return;
      }

      try {
        const initial = await fetchWorkspaceState();
        if (!disposed) this.#apply(initial);
      } catch (error) {
        // A pushed event already delivered state; the failed read is then irrelevant.
        if (!disposed && this.state === null) this.error = `Cannot read workspace state: ${errorMessage(error)}`;
      }
    })();

    // Cleanup releases whichever connection is current, including one opened by retry().
    return () => this.#disconnect?.();
  }

  /** Re-subscribes and re-reads after a connection failure. */
  retry(): void {
    this.error = null;
    this.connect();
  }
}

export const workspace = new WorkspaceStore();
