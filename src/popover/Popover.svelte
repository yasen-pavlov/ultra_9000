<script lang="ts">
  // The macOS menu-bar popover (boards 04–06): the sidebar's rows under the status item. Projects
  // and rows keep the app's order and never fold here; the row the main window shows is filled and
  // the keyboard ring is separate. Keys and clicks are commands to the app (controller.svelte.ts).
  import { tick, untrack } from "svelte";
  import { clock } from "../lib/clock.svelte";
  import Icon from "../lib/components/Icon.svelte";
  import ProjectHead from "../lib/components/ProjectHead.svelte";
  import SidebarRow from "../lib/components/SidebarRow.svelte";
  import { edgeChipLabel, measureOffscreenNeeds, sameIds } from "../lib/edges";
  import { tildePath } from "../lib/format";
  import { itemView } from "../lib/items";
  import {
    errorMessage,
    listenPopoverHidden,
    listenPopoverOpened,
    popoverCommand,
    popoverHide,
    popoverSetHeight,
    popoverToggleMain,
    type UnlistenFn,
  } from "../lib/native";
  import { settings } from "../lib/settings.svelte";
  import { workspace } from "../lib/workspace.svelte";
  import { PopoverController, rowStop } from "./controller.svelte";
  import { EMPTY_HINTS, fittingHints, LIST_HINTS, QUESTION_HINTS, type Hint } from "./hints";
  import { popoverKey } from "./keys";
  import RowConfirm from "./RowConfirm.svelte";

  let rootEl = $state<HTMLElement | null>(null);
  let headEl = $state<HTMLElement | null>(null);
  let listEl = $state<HTMLElement | null>(null);
  let innerEl = $state<HTMLElement | null>(null);
  let footEl = $state<HTMLElement | null>(null);
  let sumEl = $state<HTMLElement | null>(null);
  /** How many leading footer hints fit the band; the rest are dropped, never clipped. */
  let hintsShown = $state(Number.POSITIVE_INFINITY);
  /** The header line is too long at 320 px: show its short form. */
  let summaryShort = $state(false);
  let above = $state<string[]>([]);
  let below = $state<string[]>([]);
  let listenError = $state<string | null>(null);
  let sentHeight = 0;
  const uid = $props.id();

  const snapshot = $derived(workspace.state);
  const selectedId = $derived(snapshot?.activeItemId ?? null);
  const home = $derived(snapshot?.defaultCwd ?? "");
  const compact = $derived(settings.density === "compact");
  const groups = $derived(workspace.groups);
  const rows = $derived(groups.flatMap((group) => group.items));
  const liveProjects = $derived(groups.filter((group) => group.items.length > 0).length);
  const pinned = $derived(groups.filter((group) => group.project.pinned).length);
  /** Header line (board 05) and, when it doesn't fit, its short form: every empty project's own
   *  line already says "Nothing running", so the pinned count is what stays. */
  const summaries = $derived.by((): { full: string; short: string } => {
    if (rows.length > 0) {
      const live = `${rows.length} live · ${liveProjects} ${liveProjects === 1 ? "project" : "projects"}`;
      return { full: live, short: live };
    }
    if (pinned > 0) return { full: `nothing running · ${pinned} pinned`, short: `${pinned} pinned` };
    return { full: "nothing running", short: "nothing running" };
  });
  const summary = $derived(summaries.full);
  const shownSummary = $derived(summaryShort ? summaries.short : summaries.full);
  const needsYou = $derived(workspace.needsYouCount);
  const waiting = $derived(workspace.waiting.length);

  const controller = new PopoverController(
    () => ({
      groups: groups.map((group) => ({ projectId: group.project.id, items: group.items })),
      selectedId,
      waitingCount: waiting,
    }),
    { command: popoverCommand, hide: popoverHide, toggleMain: popoverToggleMain, reveal: focusStop },
  );

  /** The list's single tab stop: the ring's stop, else where the first key would put it. */
  const tabStop = $derived(controller.focus ?? controller.entry());
  const footer = $derived(
    controller.error !== null
      ? "error"
      : controller.question !== null
        ? "question"
        : above.length > 0 || below.length > 0
          ? "edge"
          : rows.length === 0
            ? "empty"
            : "hints",
  );
  const hints = $derived.by((): readonly Hint[] => {
    if (footer === "question") return QUESTION_HINTS;
    if (footer === "empty") return EMPTY_HINTS;
    if (footer === "hints") return LIST_HINTS;
    return [];
  });

  $effect(() => workspace.connect());
  $effect(() => settings.connect());
  $effect(() => clock.start());

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
        if (!disposed) listenError = `Cannot observe ${label}: ${errorMessage(error)}`;
      },
    );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }

  $effect(() => subscribe("the popover opening", () => listenPopoverOpened(opened)));
  $effect(() => subscribe("the popover closing", () => listenPopoverHidden(() => controller.hidden())));

  // Every snapshot: drop a question whose row ended, hand the ring on, focus a row started here.
  $effect(() => {
    void groups;
    untrack(() => controller.sync());
  });

  // DOM focus follows the controller's (keys, a vanished stop's neighbour, a new row).
  $effect(() => {
    const stop = controller.focus;
    if (stop !== null && controller.question === null) untrack(() => focusStop(stop));
  });

  /** Shown: no ring until the first key; scroll to the main window's item, or the top. */
  function opened(): void {
    controller.opened();
    above = [];
    below = [];
    // A window laid out while hidden may have measured before its fonts were ready.
    fitHints();
    void tick().then(() => {
      if (document.activeElement instanceof HTMLElement && document.activeElement !== rootEl)
        document.activeElement.blur();
      rootEl?.focus({ preventScroll: true });
      if (listEl === null) return;
      listEl.scrollTop = 0;
      if (selectedId !== null)
        listEl.querySelector(`[data-item-id="${CSS.escape(selectedId)}"]`)?.scrollIntoView({ block: "nearest" });
      measureEdges();
    });
  }

  /** Scroll a stop into view and give it DOM focus (Tab order, screen readers). The ring itself is
   *  drawn from the controller's state, so it shows after pointer actions on any WebKit. */
  function focusStop(stop: string): void {
    void tick().then(() => {
      const element = listEl?.querySelector<HTMLElement>(`[data-rove="${CSS.escape(stop)}"]`);
      element?.scrollIntoView({ block: "nearest" });
      element?.focus({ preventScroll: true });
    });
  }

  function measureEdges(): void {
    if (listEl === null) return;
    const { above: up, below: down } = measureOffscreenNeeds(listEl);
    if (!sameIds(up, above)) above = up;
    if (!sameIds(down, below)) below = down;
  }

  /** Content height after layout (border + header + list content + footer); native clamps it. */
  function measureHeight(): void {
    if (headEl === null || innerEl === null || footEl === null) return;
    const height = Math.ceil(headEl.offsetHeight + innerEl.offsetHeight + footEl.offsetHeight + 2);
    if (height === sentHeight) return;
    sentHeight = height;
    void popoverSetHeight(height).catch((error: unknown) => {
      sentHeight = 0;
      listenError = `Cannot resize the popover: ${errorMessage(error)}`;
    });
  }

  $effect(() => {
    const observed = [headEl, innerEl, footEl, listEl].filter((element) => element !== null);
    if (observed.length < 4) return;
    const observer = new ResizeObserver(() => {
      measureHeight();
      measureEdges();
    });
    for (const element of observed) observer.observe(element);
    return () => observer.disconnect();
  });

  /** Measures every hint at its natural width, then keeps the leading ones that fit the band. */
  function fitHints(): void {
    hintsShown = Number.POSITIVE_INFINITY;
    void tick().then(() => {
      if (footEl === null) return;
      const style = getComputedStyle(footEl);
      const available = footEl.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
      const widths = [...footEl.querySelectorAll<HTMLElement>("[data-hint]")].map(
        (hint) => hint.getBoundingClientRect().width,
      );
      hintsShown = fittingHints(widths, parseFloat(style.columnGap) || 0, available);
    });
  }

  $effect(() => {
    void hints;
    untrack(fitHints);
  });

  $effect(() => {
    void summary;
    summaryShort = false;
    void tick().then(() => {
      if (sumEl !== null && sumEl.scrollWidth > sumEl.clientWidth) summaryShort = true;
    });
  });

  // Rows change state in place; needs-you rows may enter or leave the hidden part.
  $effect(() => {
    void rows;
    const frame = requestAnimationFrame(measureEdges);
    return () => cancelAnimationFrame(frame);
  });

  function onkeydown(event: KeyboardEvent): void {
    if (event.defaultPrevented) return;
    const key = popoverKey(event);
    if (key === null) return;
    // Enter on a button that is not the ring's stop (header, footer, question) activates that
    // button; a held Enter is still swallowed so it never activates it again.
    const target = event.target;
    if (
      key.kind === "enter" &&
      !event.repeat &&
      target instanceof HTMLButtonElement &&
      target.dataset.rove !== controller.focus
    )
      return;
    if (controller.key(key, event.repeat)) event.preventDefault();
  }

  function onfocusin(event: FocusEvent): void {
    const rove = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-rove]")?.dataset.rove;
    if (rove !== undefined) controller.focused(rove);
  }
</script>

<svelte:window {onkeydown} />

<div class="pop" tabindex="-1" bind:this={rootEl} aria-label="Ultra 9000">
  <header class="phd" bind:this={headEl}>
    <svg class="appi" viewBox="0 0 32 32" aria-hidden="true"
      ><path fill="#212121" d="M7 0H25A7 7 0 0 1 32 7V25A7 7 0 0 1 25 32H7A7 7 0 0 1 0 25V7A7 7 0 0 1 7 0Z" /><path
        fill="none"
        stroke="#3b3b3b"
        d="M7 .5H25A6.5 6.5 0 0 1 31.5 7V25A6.5 6.5 0 0 1 25 31.5H7A6.5 6.5 0 0 1 .5 25V7A6.5 6.5 0 0 1 7 .5Z"
      /><path fill="#eeffff" d="M4 6h4v12a4 4 0 0 0 8 0V6h4v12a8 8 0 0 1-16 0Z" /><path
        fill="#82aaff"
        d="M22 22h6v4h-6Z"
      /></svg
    >
    <div class="hd">
      <span class="ttl">Ultra 9000</span>
      <span class="sum" bind:this={sumEl} title={summaryShort ? summary : undefined}
        >{snapshot === null ? "" : shownSummary}</span
      >
    </div>
    <div class="acts">
      <button
        type="button"
        class="btn sm ghost"
        disabled={waiting === 0}
        aria-label={needsYou > 0
          ? `Jump to waiting, ${needsYou} ${needsYou === 1 ? "needs" : "need"} you`
          : "Jump to waiting"}
        title="Jump to waiting (j)"
        onclick={() => controller.jump()}
        ><Icon name="jump" />Jump{#if needsYou > 0}<span class="cnt">{needsYou}</span>{/if}</button
      >
      <button
        type="button"
        class="iconbtn"
        aria-label="Open palette"
        title="Open palette (o)"
        onclick={() => controller.palette()}><Icon name="search" /></button
      >
      <button
        type="button"
        class="iconbtn"
        aria-label="Show or hide Ultra 9000"
        title="Show or hide Ultra 9000 (h)"
        onclick={() => controller.toggleMain()}><Icon name="window" /></button
      >
    </div>
  </header>

  <div class="plist scroll" bind:this={listEl} onscroll={measureEdges} {onfocusin}>
    <div class="inner" bind:this={innerEl}>
      {#if snapshot === null}
        <div class="pnone">
          {#if workspace.error !== null}
            <span class="es err">{workspace.error}</span>
          {:else}
            <span class="es">Connecting…</span>
          {/if}
        </div>
      {:else if groups.length === 0}
        <div class="pnone">
          <span class="et">Nothing running</span>
          <span class="es">Start something here. It runs in the app; press ↵ on its row to show it.</span>
          <div class="eb">
            <button type="button" class="btn sm" onclick={() => controller.newItem("agent")}
              >New agent<kbd>n</kbd></button
            >
            <button type="button" class="btn sm" onclick={() => controller.newItem("shell")}
              >Terminal<kbd>t</kbd></button
            >
            <button type="button" class="btn sm" onclick={() => controller.newItem("neovim")}>Neovim<kbd>e</kbd></button
            >
            <button type="button" class="btn sm" onclick={() => controller.palette()}>Palette<kbd>o</kbd></button>
          </div>
        </div>
      {:else}
        {#each groups as group, index (group.project.id)}
          {@const project = group.project}
          {@const labelId = `${uid}-project-${index}`}
          <div class="grp" role="group" aria-labelledby={labelId}>
            <ProjectHead
              {project}
              items={group.items}
              roveKey={tabStop}
              {labelId}
              onnew={() => controller.palette({ id: project.id, mode: "new" })}
            />
            {#if project.missing}
              <div class="pempty missing">Folder missing · {tildePath(project.path, home)}</div>
            {:else if group.items.length === 0}
              <div class="pempty">Nothing running · + to start</div>
            {/if}
            {#if group.items.length > 0}
              <div class="rows" role="list" aria-labelledby={labelId}>
                {#each group.items as item (item.id)}
                  <div role="listitem">
                    {#if controller.question?.itemId === item.id}
                      <RowConfirm
                        {item}
                        reason={controller.question.reason}
                        onkeep={() => controller.cancel()}
                        onend={() => controller.confirm()}
                      />
                    {:else}
                      {@const view = itemView(item, { tool: workspace.tool(item.tool), now: clock.now, home })}
                      {@const dis = workspace.disambiguation.get(item.id) ?? ""}
                      {@const stop = rowStop(item.id)}
                      <SidebarRow
                        {item}
                        {view}
                        {dis}
                        label="{view.title}{dis}, {project.name}, {workspace.toolLabel(item.tool)}, {view.stateText}"
                        selected={item.id === selectedId}
                        {compact}
                        rovable={tabStop === stop}
                        ring={controller.focus === stop}
                        onselect={() => controller.select(item.id)}
                        onclose={() => {
                          controller.focused(stop);
                          void controller.close(item.id);
                        }}
                      />
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      {/if}
    </div>
  </div>

  <footer class="pfoot" bind:this={footEl}>
    {#if footer === "error"}
      <span class="err" role="alert" title={controller.error}>{controller.error}</span>
    {:else if footer === "edge"}
      {#if above.length > 0}
        <button type="button" class="chip need" onclick={() => controller.reveal(above[above.length - 1])}
          >{edgeChipLabel("up", above.length)}</button
        >
      {/if}
      {#if below.length > 0}
        <button type="button" class="chip need" onclick={() => controller.reveal(below[0])}
          >{edgeChipLabel("down", below.length)}</button
        >
      {/if}
    {:else}
      {#each hints as hint, index (hint.label)}
        <span data-hint hidden={index >= hintsShown}
          >{#each hint.keys as key, at (at)}<kbd>{key}</kbd>{/each}{hint.label}</span
        >
      {/each}
    {/if}
    {#if listenError !== null && footer !== "error"}<span class="visually-hidden" role="alert">{listenError}</span>{/if}
  </footer>
</div>

<style>
  /* The window is transparent; the popover draws its own rounded surface (native adds the shadow). */
  :global(html.popover),
  :global(html.popover body) {
    background: transparent;
  }
  .pop {
    --pop-radius: 12px;
    --pop-header: 46px;
    --pop-footer: 34px;
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--chrome);
    border: 1px solid var(--outline);
    border-radius: var(--pop-radius);
    overflow: hidden;
    color: var(--text);
  }
  .pop:focus-visible {
    outline: none;
  }
  .phd {
    display: flex;
    align-items: center;
    gap: 9px;
    height: var(--pop-header);
    padding: 0 8px 0 14px;
    flex: none;
  }
  .appi {
    width: 20px;
    height: 20px;
    display: block;
    flex: none;
  }
  .hd {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .ttl {
    font-weight: 600;
    font-size: 13px;
    line-height: 17px;
    color: var(--hi);
    white-space: nowrap;
  }
  .sum {
    font-size: 11.5px;
    line-height: 15px;
    color: var(--dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .acts {
    margin-left: auto;
    display: flex;
    gap: 2px;
    align-items: center;
  }
  .cnt {
    height: 16px;
    min-width: 16px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--accent);
    color: #1d1200;
    font-weight: 650;
    font-size: 10.5px;
    line-height: 16px;
    text-align: center;
  }
  .plist {
    flex: 1;
    min-height: 0;
  }
  /* Semantic wrappers only: the project heads, empty lines and rows lay out as one flat column,
     as in the sidebar (the 10 px gap above every project head but the first). */
  .grp,
  .rows {
    display: flex;
    flex-direction: column;
    flex: none;
  }
  .inner .grp + .grp > :global(.proj) {
    margin-top: 10px;
  }
  .inner {
    padding: 0 8px 6px;
    display: flex;
    flex-direction: column;
  }
  .pempty {
    padding: 4px 8px 6px 10px;
    font-size: 11.5px;
    line-height: 16px;
    color: #6d6d6d;
    flex: none;
  }
  .pempty.missing {
    color: var(--yellow);
  }
  .pnone {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 6px;
    padding: 26px 20px 22px;
  }
  .et {
    font-weight: 600;
    font-size: 13px;
    line-height: 18px;
    color: var(--hi);
  }
  .es {
    font-size: 11.5px;
    line-height: 16px;
    color: var(--dim);
    max-width: 250px;
  }
  .es.err {
    color: var(--err-text);
  }
  .eb {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 6px;
    margin-top: 10px;
    width: 100%;
  }
  .eb .btn {
    justify-content: space-between;
  }
  .pfoot {
    flex: none;
    height: var(--pop-footer);
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 0 12px;
    border-top: 1px solid var(--line);
    font-size: 11.5px;
    line-height: 1;
    color: var(--dim);
    white-space: nowrap;
  }
  .pfoot > span {
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
  .pfoot .chip + .chip {
    margin-left: -4px;
  }
  .pfoot .err {
    display: block;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--err-text);
  }
</style>
