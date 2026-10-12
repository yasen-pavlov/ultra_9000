<script lang="ts">
  // Running items grouped by project: expanded (rows) or collapsed (48 px strip of tiles with
  // project initials). Roving focus inside the list: arrows move, Enter selects, Left/Right fold,
  // Delete closes, Esc returns to the terminal. Order is native and never changes with state.
  import { clock } from "../clock.svelte";
  import { run } from "../errors.svelte";
  import { edgeChipLabel, measureOffscreenNeeds, sameIds } from "../edges";
  import { projectInitials } from "../format";
  import { itemView } from "../items";
  import { itemClose, itemSelect, sheetClose, sheetOpen, uiSet, type ItemState } from "../native";
  import { settings } from "../settings.svelte";
  import { requestTerminalFocus } from "../viewport";
  import { workspace } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import NoticeList from "./NoticeList.svelte";
  import ProjectGroup from "./ProjectGroup.svelte";
  import ToolTile from "./ToolTile.svelte";

  let {
    expanded,
    viewportError,
    ontoggle,
    onpeek,
  }: {
    expanded: boolean;
    viewportError: string | null;
    ontoggle: () => void;
    /** Hover/focus preview of a collapsed-strip item in the context bar (null ends it). */
    onpeek: (itemId: string | null) => void;
  } = $props();

  let listEl = $state<HTMLElement | null>(null);
  let roveKey = $state<string | null>(null);
  let above = $state<string[]>([]);
  let below = $state<string[]>([]);
  let announcement = $state("");
  let announced = new Set<string>();

  const snapshot = $derived(workspace.state);
  const activeId = $derived(snapshot?.activeItemId ?? null);
  const sheetKind = $derived(snapshot?.sheet?.kind ?? null);
  const home = $derived(snapshot?.defaultCwd ?? "");

  /** Every roving stop of the current mode, in DOM order. */
  const roveKeys = $derived.by(() => {
    const keys: string[] = [];
    for (const group of workspace.groups) {
      if (expanded) {
        keys.push(`head:${group.project.id}`, `plus:${group.project.id}`);
        if (!group.folded) for (const item of group.items) keys.push(`row:${item.id}`);
      } else {
        keys.push(`cproj:${group.project.id}`);
        if (!group.folded) for (const item of group.items) keys.push(`ic:${item.id}`);
      }
    }
    keys.push(expanded ? "new" : "cnew");
    return keys;
  });

  /** The single tab stop of the list: the last focused entry, else the selected item, else the first. */
  const tabStop = $derived.by(() => {
    if (roveKey !== null && roveKeys.includes(roveKey)) return roveKey;
    const selected = activeId === null ? null : `${expanded ? "row" : "ic"}:${activeId}`;
    if (selected !== null && roveKeys.includes(selected)) return selected;
    return roveKeys[0] ?? null;
  });

  const waitingText = $derived(
    workspace.needsYouCount > 0
      ? `${workspace.needsYouCount} ${workspace.needsYouCount === 1 ? "needs" : "need"} you`
      : `${workspace.unreadCount} your turn`,
  );

  // Needs-you changes are announced once, politely.
  $effect(() => {
    const needs = workspace.waiting.filter((item) => item.activity === "needs-you");
    const fresh = needs.filter((item) => !announced.has(item.id));
    announced = new Set(needs.map((item) => item.id));
    if (fresh.length > 0) announcement = fresh.map((item) => `${item.title} needs you`).join(". ");
  });

  function measureEdges(): void {
    if (listEl === null) return;
    const { above: up, below: down } = measureOffscreenNeeds(listEl);
    if (!sameIds(up, above)) above = up;
    if (!sameIds(down, below)) below = down;
  }

  // Waiting items scrolled out of view get an edge chip.
  $effect(() => {
    const element = listEl;
    if (element === null) return;
    void workspace.groups;
    void settings.density;
    const frame = requestAnimationFrame(measureEdges);
    const observer = new ResizeObserver(measureEdges);
    observer.observe(element);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  });

  // Keep the selected item visible when selection moves (shortcuts, palette, jump).
  $effect(() => {
    const id = activeId;
    if (listEl === null || id === null) return;
    listEl.querySelector(`[data-item-id="${CSS.escape(id)}"]`)?.scrollIntoView({ block: "nearest" });
  });

  function select(item: ItemState): void {
    void run("Select", () => itemSelect(item.id));
  }

  function close(item: ItemState): void {
    void run("Close", () => itemClose(item.id, false));
  }

  function fold(projectId: string, folded: boolean): void {
    const current = snapshot?.ui.foldedProjectIds ?? [];
    if (current.includes(projectId) === folded) return;
    const next = folded ? [...current, projectId] : current.filter((id) => id !== projectId);
    void run("Fold", () => uiSet({ foldedProjectIds: next }));
  }

  function openPalette(projectId: string | null, mode: "open" | "new"): void {
    void run("Open", () => sheetOpen({ kind: "palette", projectId, mode }));
  }

  function toggleSheet(kind: "settings" | "shortcuts"): void {
    void run(kind === "settings" ? "Settings" : "Shortcuts", () =>
      sheetKind === kind ? sheetClose() : sheetOpen({ kind }),
    );
  }

  /** Needs-you first, then unread your-turn, oldest first; repeated use cycles. */
  function jumpWaiting(): void {
    const list = workspace.waiting;
    if (list.length === 0) return;
    const at = list.findIndex((item) => item.id === activeId);
    select(list[(at + 1) % list.length]);
  }

  function jumpTo(id: string | undefined): void {
    const item = id === undefined ? undefined : workspace.itemsById.get(id);
    if (item !== undefined) select(item);
  }

  /** Native moved keyboard focus to the web view (focus-sidebar); take it into the list. */
  export function focusList(): void {
    const container = listEl;
    if (container === null) return;
    const stop = tabStop === null ? null : container.querySelector<HTMLElement>(`[data-rove="${CSS.escape(tabStop)}"]`);
    (stop ?? container.querySelector<HTMLElement>("[data-rove]"))?.focus();
  }

  function onfocusin(event: FocusEvent): void {
    const entry = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-rove]");
    if (entry?.dataset.rove !== undefined) roveKey = entry.dataset.rove;
  }

  function onkeydown(event: KeyboardEvent): void {
    if (listEl === null || event.ctrlKey || event.altKey || event.metaKey) return;
    const entry = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-rove]");
    const key = entry?.dataset.rove;
    if (entry == null || key === undefined) return;
    const entries = [...listEl.querySelectorAll<HTMLElement>("[data-rove]")];
    const index = entries.indexOf(entry);
    const [kind, ...rest] = key.split(":");
    const id = rest.join(":");
    const focusAt = (at: number) => entries[Math.max(0, Math.min(entries.length - 1, at))]?.focus();
    switch (event.key) {
      case "ArrowDown":
        focusAt(index + 1);
        break;
      case "ArrowUp":
        focusAt(index - 1);
        break;
      case "Home":
        focusAt(0);
        break;
      case "End":
        focusAt(entries.length - 1);
        break;
      case "ArrowLeft":
        if (kind === "head" || kind === "cproj") fold(id, true);
        else if (kind === "row" || kind === "ic") {
          const projectId = workspace.itemsById.get(id)?.projectId;
          if (projectId === undefined) return;
          fold(projectId, true);
          roveKey = `${kind === "row" ? "head" : "cproj"}:${projectId}`;
          listEl.querySelector<HTMLElement>(`[data-rove="${CSS.escape(roveKey)}"]`)?.focus();
        } else return;
        break;
      case "ArrowRight":
        if (kind !== "head" && kind !== "cproj") return;
        fold(id, false);
        break;
      case "Delete": {
        const item = kind === "row" || kind === "ic" ? workspace.itemsById.get(id) : undefined;
        if (item === undefined) return;
        close(item);
        break;
      }
      case "Escape":
        entry.blur();
        if (workspace.terminalVisible) requestTerminalFocus();
        else document.querySelector<HTMLElement>("[data-sheet-autofocus]")?.focus();
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  }
</script>

<div class="visually-hidden" aria-live="polite">{announcement}</div>

{#if expanded}
  <aside class="sb" style="width: {settings.sidebarWidth}px" aria-label="Running items" data-sidebar>
    <button type="button" class="openbtn" class:on={sheetKind === "palette"} onclick={() => openPalette(null, "open")}>
      <Icon name="search" />Open…{#if workspace.shortcut("palette") !== null}<kbd>{workspace.shortcut("palette")}</kbd
        >{/if}
    </button>
    <div class="listwrap">
      <div
        class="list scroll"
        role="toolbar"
        tabindex="-1"
        aria-orientation="vertical"
        aria-label="Projects"
        bind:this={listEl}
        onscroll={measureEdges}
        {onfocusin}
        {onkeydown}
      >
        {#each workspace.groups as group (group.project.id)}
          <ProjectGroup
            {group}
            roveKey={tabStop}
            onfold={() => fold(group.project.id, !group.folded)}
            onnew={() => openPalette(group.project.id, "new")}
            onselect={select}
            onclose={close}
          />
        {/each}
        <button
          type="button"
          class="newrow"
          data-rove="new"
          tabindex={tabStop === "new" ? 0 : -1}
          onclick={() => openPalette(null, "new")}
        >
          <Icon name="plus" />New…{#if workspace.shortcut("new-agent") !== null}<kbd
              >{workspace.shortcut("new-agent")}</kbd
            >{/if}
        </button>
      </div>
      {#if above.length > 0}
        <div class="edgechip top">
          <button type="button" class="chip need" onclick={() => jumpTo(above[above.length - 1])}>
            {edgeChipLabel("up", above.length)}
          </button>
        </div>
      {/if}
      {#if below.length > 0}
        <div class="edgechip bottom">
          <button type="button" class="chip need" onclick={() => jumpTo(below[0])}>
            {edgeChipLabel("down", below.length)}
          </button>
        </div>
      {/if}
    </div>
    <NoticeList {viewportError} />
    <div class="util">
      <button
        type="button"
        class="iconbtn"
        class:on={sheetKind === "settings"}
        aria-label={settings.hasErrors ? "Settings (a settings file has an error)" : "Settings"}
        title="Settings"
        onclick={() => toggleSheet("settings")}
      >
        <Icon name="settings" />{#if settings.hasErrors}<span class="errdot"></span>{/if}
      </button>
      <button
        type="button"
        class="iconbtn"
        class:on={sheetKind === "shortcuts"}
        aria-label="Keyboard shortcuts"
        title="Keyboard shortcuts"
        onclick={() => toggleSheet("shortcuts")}
      >
        <Icon name="keyboard" />
      </button>
      <span class="sp" data-tauri-drag-region></span>
      {#if workspace.waiting.length > 0}
        <!-- Orange is reserved for needs-you (board 21); unread your-turn alone stays neutral. -->
        <button type="button" class="chip" class:need={workspace.needsYouCount > 0} onclick={jumpWaiting}>
          {waitingText}{#if workspace.shortcut("jump-waiting") !== null}<kbd>{workspace.shortcut("jump-waiting")}</kbd
            >{/if}
        </button>
      {/if}
      <button type="button" class="iconbtn" aria-label="Collapse sidebar" title="Collapse sidebar" onclick={ontoggle}>
        <Icon name="sidebar" />
      </button>
    </div>
  </aside>
{:else}
  <aside class="sb c" aria-label="Running items" data-sidebar>
    <button
      type="button"
      class="ic"
      class:sel={sheetKind === "palette"}
      aria-label="Open{workspace.shortcut('palette') === null ? '' : ` (${workspace.shortcut('palette')})`}"
      title="Open…"
      onclick={() => openPalette(null, "open")}
    >
      <Icon name="search" />
    </button>
    <div class="listwrap">
      <div
        class="list strip scroll"
        role="toolbar"
        tabindex="-1"
        aria-orientation="vertical"
        aria-label="Projects"
        bind:this={listEl}
        onscroll={measureEdges}
        {onfocusin}
        {onkeydown}
      >
        {#each workspace.groups as group (group.project.id)}
          {@const waiting = group.folded && group.items.some((item) => item.activity === "needs-you")}
          <button
            type="button"
            class="cproj"
            class:folded={group.folded}
            data-rove="cproj:{group.project.id}"
            tabindex={tabStop === `cproj:${group.project.id}` ? 0 : -1}
            aria-label="New or resume in {group.project.name}{group.folded
              ? `, folded, ${group.items.length} running`
              : ''}"
            title={group.project.name}
            onclick={() => openPalette(group.project.id, "new")}
          >
            {projectInitials(group.project.name)}{#if waiting}<span class="cdot"></span>{/if}
          </button>
          {#if !group.folded}
            {#each group.items as item (item.id)}
              {@const view = itemView(item, { tool: workspace.tool(item.tool), now: clock.now, home })}
              <button
                type="button"
                class="ic"
                class:sel={item.id === activeId}
                data-rove="ic:{item.id}"
                data-item-id={item.id}
                data-need={item.activity === "needs-you" ? "" : undefined}
                tabindex={tabStop === `ic:${item.id}` ? 0 : -1}
                aria-label="{view.title}{workspace.disambiguation.get(item.id) ?? ''}, {group.project
                  .name}, {workspace.toolLabel(item.tool)}, {view.stateText}"
                aria-current={item.id === activeId ? "true" : undefined}
                onclick={() => select(item)}
                onmouseenter={() => onpeek(item.id)}
                onmouseleave={() => onpeek(null)}
                onfocus={() => onpeek(item.id)}
                onblur={() => onpeek(null)}
              >
                <ToolTile
                  tool={item.tool}
                  model={item.model}
                  badge={view.badge}
                  badgeGlyph={view.badgeGlyph}
                  unread={item.unread}
                />
              </button>
            {/each}
          {/if}
        {/each}
        <button
          type="button"
          class="ic newic"
          data-rove="cnew"
          tabindex={tabStop === "cnew" ? 0 : -1}
          aria-label="New{workspace.shortcut('new-agent') === null ? '' : ` (${workspace.shortcut('new-agent')})`}"
          title="New…"
          onclick={() => openPalette(null, "new")}
        >
          <Icon name="plus" />
        </button>
      </div>
      {#if above.length > 0}
        <div class="more top">
          <button
            type="button"
            aria-label="{above.length} waiting above"
            onclick={() => jumpTo(above[above.length - 1])}>↑ {above.length}</button
          >
        </div>
      {/if}
      {#if below.length > 0}
        <div class="more bottom">
          <button type="button" aria-label="{below.length} waiting below" onclick={() => jumpTo(below[0])}
            >↓ {below.length}</button
          >
        </div>
      {/if}
    </div>
    <div class="util">
      <button
        type="button"
        class="iconbtn"
        class:on={sheetKind === "settings"}
        aria-label={settings.hasErrors ? "Settings (a settings file has an error)" : "Settings"}
        title="Settings"
        onclick={() => toggleSheet("settings")}
      >
        <Icon name="settings" />{#if settings.hasErrors}<span class="errdot"></span>{/if}
      </button>
      <button
        type="button"
        class="iconbtn help"
        class:on={sheetKind === "shortcuts"}
        aria-label="Keyboard shortcuts"
        title="Keyboard shortcuts"
        onclick={() => toggleSheet("shortcuts")}
      >
        <Icon name="keyboard" />
      </button>
      <button type="button" class="iconbtn" aria-label="Expand sidebar" title="Expand sidebar" onclick={ontoggle}>
        <Icon name="sidebar" />
      </button>
    </div>
  </aside>
{/if}

<style>
  .sb {
    flex: none;
    display: flex;
    flex-direction: column;
    padding: 8px 6px 6px 8px;
    gap: 6px;
    min-height: 0;
    height: 100%;
  }
  .sb.c {
    width: 48px;
    padding: 8px 6px;
    align-items: center;
  }
  .openbtn {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 8px 0 10px;
    border-radius: 8px;
    background: #1f1f1f;
    border: 1px solid var(--line);
    color: var(--dim);
    font-size: 13px;
    width: 100%;
    text-align: left;
    flex: none;
  }
  .openbtn:hover,
  .openbtn.on {
    border-color: #4a4a4a;
    color: var(--hi);
  }
  .openbtn kbd {
    margin-left: auto;
  }
  .listwrap {
    flex: 1;
    min-height: 0;
    position: relative;
    display: flex;
    flex-direction: column;
    width: 100%;
  }
  .list {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-height: 0;
    scrollbar-width: none;
  }
  .list::-webkit-scrollbar {
    display: none;
  }
  .list.strip {
    align-items: center;
    gap: 2px;
    margin-top: 4px;
  }
  .newrow {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 32px;
    padding: 0 8px 0 10px;
    border-radius: 8px;
    color: var(--dim);
    width: 100%;
    text-align: left;
    font-size: 13px;
    flex: none;
    margin-top: 8px;
  }
  .newrow:hover {
    background: var(--hover);
    color: var(--hi);
  }
  .newrow kbd {
    margin-left: auto;
  }
  .ic {
    width: 36px;
    height: 36px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    position: relative;
    flex: none;
    color: var(--dim);
  }
  /* --row-bg feeds the tile's provider plate and badge rings; selection wins over hover. */
  .ic:hover {
    --row-bg: var(--hover);
    background: var(--row-bg);
    color: var(--hi);
  }
  .ic.sel {
    --row-bg: var(--sel);
    background: var(--row-bg);
    color: var(--hi);
  }
  .ic:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 2px var(--blue);
  }
  .newic {
    margin-top: 6px;
  }
  .cproj {
    width: 36px;
    height: 18px;
    display: grid;
    place-items: center;
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: #777;
    border-top: 1px solid #2c2c2c;
    margin-top: 6px;
    padding-top: 4px;
    box-sizing: content-box;
    text-transform: uppercase;
    flex: none;
    position: relative;
  }
  .cproj:first-child {
    border-top: 0;
    margin-top: 0;
  }
  .cproj:hover {
    color: var(--hi);
  }
  .cproj.folded {
    color: var(--text);
  }
  .cdot {
    position: absolute;
    right: 2px;
    top: 6px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }
  .edgechip {
    position: absolute;
    left: 0;
    right: 0;
    display: flex;
    justify-content: center;
    z-index: 2;
    pointer-events: none;
  }
  .edgechip .chip {
    pointer-events: auto;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.5);
  }
  .edgechip.top,
  .more.top {
    top: 0;
  }
  .edgechip.bottom,
  .more.bottom {
    bottom: 0;
  }
  .more {
    position: absolute;
    left: 0;
    right: 0;
    display: flex;
    justify-content: center;
    padding: 2px 0;
    z-index: 2;
    pointer-events: none;
  }
  .more.bottom {
    padding-top: 14px;
    background: linear-gradient(transparent, var(--chrome) 45%);
  }
  .more.top {
    padding-bottom: 14px;
    background: linear-gradient(var(--chrome) 55%, transparent);
  }
  .more button {
    pointer-events: auto;
    height: 20px;
    min-width: 30px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--accent);
    color: #1d1200;
    font-size: 10.5px;
    font-weight: 700;
  }
  .util {
    display: flex;
    align-items: center;
    gap: 2px;
    padding-top: 6px;
    border-top: 1px solid #262626;
    flex: none;
    width: 100%;
  }
  .sb.c .util {
    flex-direction: column;
    align-items: center;
  }
  .util .iconbtn {
    width: 30px;
    height: 30px;
    border-radius: 7px;
  }
  .util .sp {
    flex: 1;
    align-self: stretch;
  }
  .util .chip {
    margin-right: 2px;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
  }
  .errdot {
    position: absolute;
    right: 5px;
    top: 5px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--err);
  }
  /* Small tiles (board 07, 640×400): 32 px targets with 22 px tiles and only the utilities that
     have no other mouse path, so about two more items fit before the "↓ N" chip. Shortcuts stay
     on their key; Expand stays because a narrow window ignores the Settings sidebar toggle. */
  @media (max-width: 760px), (max-height: 520px) {
    .sb.c {
      padding-top: 6px;
    }
    .sb.c .ic {
      height: 32px;
    }
    .sb.c .ic :global(.tile) {
      width: 22px;
      height: 22px;
    }
    .sb.c .ic :global(.tile > .m) {
      left: 4px;
      top: 4px;
    }
    .sb.c .help {
      display: none;
    }
  }
</style>
