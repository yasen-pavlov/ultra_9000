<script lang="ts">
  // One palette for switch, resume and start (also the cold-start launcher). Running rows switch;
  // saved rows resume as a new process; the launch line starts something new. Keys: ↑↓ move,
  // ↵ open the row, Ctrl+↵ start new, Tab walks the launch line, Backspace on an empty query drops
  // the project scope, Esc closes a menu first, then the palette.
  import { untrack } from "svelte";
  import { clock } from "../clock.svelte";
  import { conversations } from "../conversations.svelte";
  import { run } from "../errors.svelte";
  import {
    calendarDay,
    clockTime,
    highlight,
    matchesQuery,
    modelShort,
    recency,
    recencyBucket,
    spokenAge,
    tildePath,
  } from "../format";
  import { itemView } from "../items";
  import {
    conversationsList,
    errorMessage,
    itemLaunch,
    itemResume,
    itemSelect,
    lastSetDismiss,
    lastSetResume,
    sheetClose,
    type Conversation,
    type ConversationList,
    type ItemState,
  } from "../native";
  import { workspace } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import LaunchLine from "./LaunchLine.svelte";
  import PaletteDetail from "./PaletteDetail.svelte";
  import ProviderMark from "./ProviderMark.svelte";
  import ToolTile from "./ToolTile.svelte";

  let {
    variant,
    initialScope,
    mode,
  }: {
    /** "launcher": the empty state when nothing is selected (not a sheet; Esc has nowhere to go). */
    variant: "sheet" | "launcher";
    initialScope: string | null;
    mode: "open" | "new";
  } = $props();

  const SEARCH_LIMIT = 200;
  const SAVED_LIMIT = 200;
  const WIDE = 860;

  type Entry =
    | { key: string; kind: "launch" }
    | { key: string; kind: "lastset" }
    | { key: string; kind: "lastset-dismiss" }
    | { key: string; kind: "run"; item: ItemState }
    | { key: string; kind: "conv"; conversation: Conversation };

  interface SavedSection {
    /** Recency bucket while searching, else "" (one flat list); unique among sections. */
    title: string;
    rows: Conversation[];
  }

  // The search input is a combobox over one listbox of every Enter target; the cursor is its
  // active descendant, so focus never leaves the input.
  const uid = $props.id();
  const listId = `${uid}-results`;

  /** Stable DOM id per entry key (keys hold paths); "." never appears literally, so it is injective. */
  function optionId(key: string): string {
    return `${uid}-opt-${key.replace(/[^A-Za-z0-9_-]/g, (char) => `.${char.charCodeAt(0).toString(16)}.`)}`;
  }

  let query = $state("");
  let scope = $state<string | null>(untrack(() => initialScope));
  let chosen = $state<string | null>(null);
  let guard = $state<Conversation | null>(null);
  let search = $state<{ key: string; list: ConversationList | null; error: string | null }>({
    key: "",
    list: null,
    error: null,
  });
  let width = $state(0);
  let showStorePaths = $state(false);
  let root = $state<HTMLElement | null>(null);
  let input = $state<HTMLInputElement | null>(null);
  let results = $state<HTMLElement | null>(null);
  let guardCancel = $state<HTMLButtonElement | null>(null);
  let launchLine = $state<ReturnType<typeof LaunchLine> | null>(null);

  const home = $derived(workspace.state?.defaultCwd ?? "");
  const active = $derived(workspace.active);
  const q = $derived(query.trim());
  const scopeName = $derived(scope === null ? "" : workspace.projectName(scope));
  const searchKey = $derived(`${scope ?? ""}\n${q}`);
  const lastSet = $derived(workspace.state?.lastSet ?? null);

  // Saved conversations stay fresh while this palette is mounted.
  $effect(() => conversations.watch());

  // The search input owns the keyboard on open, so typing never reaches a terminal.
  $effect(() => {
    untrack(() => input?.focus());
  });

  // Native search (title, project name, model); debounced, stale answers dropped.
  $effect(() => {
    const key = searchKey;
    const text = q;
    const projectId = scope;
    void conversations.changes;
    const timer = setTimeout(
      () => {
        conversationsList({ ...(projectId === null ? {} : { projectId }), query: text, limit: SEARCH_LIMIT }).then(
          (list) => {
            if (searchKey === key) search = { key, list, error: null };
          },
          (error) => {
            if (searchKey === key) search = { key, list: null, error: `Search failed: ${errorMessage(error)}` };
          },
        );
      },
      text === "" ? 0 : 120,
    );
    return () => clearTimeout(timer);
  });

  const running: ItemState[] = $derived(
    workspace.groups
      .flatMap((group) => group.items)
      .filter(
        (item) =>
          (scope === null || item.projectId === scope) &&
          matchesQuery(
            q,
            item.title,
            workspace.projectName(item.projectId),
            workspace.toolLabel(item.tool),
            item.model,
          ),
      ),
  );

  const inScope: Conversation[] = $derived(
    (conversations.list?.conversations ?? []).filter(
      (conversation) => scope === null || conversation.projectId === scope,
    ),
  );
  const searching = $derived(search.key !== searchKey);
  /** Saved conversations to show: never ones running here (those are under Running). */
  const saved: Conversation[] = $derived(
    (search.key === searchKey ? (search.list?.conversations ?? []) : []).filter(
      (conversation) => conversation.runningItemId === null,
    ),
  );
  const savedTotal = $derived(inScope.filter((conversation) => conversation.runningItemId === null).length);

  const sections: SavedSection[] = $derived.by(() => {
    const shown = saved.slice(0, SAVED_LIMIT);
    // No query: one flat list, newest first (boards 09, 10). A search groups by recency (board 11).
    if (q === "") return shown.length === 0 ? [] : [{ title: "", rows: shown }];
    const buckets = new Map<string, SavedSection>();
    for (const conversation of shown) {
      const title = recencyBucket(conversation.updatedAt, clock.now);
      const bucket = buckets.get(title);
      if (bucket === undefined) buckets.set(title, { title, rows: [conversation] });
      else bucket.rows.push(conversation);
    }
    return [...buckets.values()];
  });
  const savedLabel = $derived(
    q === "" && scope === null ? "Recent conversations" : scope === null ? "Saved" : `Saved in ${scopeName}`,
  );

  /** Start-date suffix for saved conversations sharing a title. */
  const savedDis: ReadonlyMap<string, string> = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const conversation of saved) counts.set(conversation.title, (counts.get(conversation.title) ?? 0) + 1);
    const suffixes = new Map<string, string>();
    for (const conversation of saved) {
      if ((counts.get(conversation.title) ?? 0) > 1) {
        suffixes.set(
          conversation.path,
          ` · ${calendarDay(conversation.createdAt, clock.now)} ${clockTime(conversation.createdAt)}`,
        );
      }
    }
    return suffixes;
  });

  const entries: Entry[] = $derived.by(() => {
    const list: Entry[] = [{ key: "launch", kind: "launch" }];
    if (lastSet !== null && lastSet.length > 0) {
      list.push({ key: "lastset", kind: "lastset" }, { key: "lastset-dismiss", kind: "lastset-dismiss" });
    }
    for (const item of running) list.push({ key: `run:${item.id}`, kind: "run", item });
    for (const section of sections) {
      for (const conversation of section.rows) {
        list.push({ key: `conv:${conversation.path}`, kind: "conv", conversation });
      }
    }
    return list;
  });
  const entryByKey: ReadonlyMap<string, Entry> = $derived(new Map(entries.map((entry) => [entry.key, entry])));

  /** With a query: the best match. Opened for "open": the previous item, so ↵ toggles back. */
  const defaultCursor = $derived.by(() => {
    if (q !== "") return entries.find((entry) => entry.kind === "run" || entry.kind === "conv")?.key ?? "launch";
    if (mode === "open" && workspace.previousItemId !== null) {
      const previous = `run:${workspace.previousItemId}`;
      if (entryByKey.has(previous)) return previous;
    }
    return "launch";
  });
  const cursor = $derived(chosen !== null && entryByKey.has(chosen) ? chosen : defaultCursor);
  const cursorEntry = $derived(entryByKey.get(cursor) ?? null);

  // Keep the cursor row visible (the launch line sits above the results).
  $effect(() => {
    const key = cursor;
    if (key === "launch") return;
    results?.querySelector(`#${CSS.escape(optionId(key))}`)?.scrollIntoView({ block: "nearest" });
  });

  const launchTool = $derived(launchLine?.toolName() ?? "");
  const launchName = $derived(
    `Start ${launchTool === "" ? "" : `${launchTool} `}in ${launchLine?.targetName() ?? scopeName}`,
  );

  // The guard step's safe default is Cancel.
  $effect(() => {
    if (guard !== null) guardCancel?.focus();
  });

  const title = $derived(
    variant === "launcher" || (workspace.state?.items.length ?? 0) === 0
      ? "Start or resume"
      : scope === null
        ? "Open"
        : `Open in ${scopeName}`,
  );

  const lastSetSummary = $derived.by(() => {
    if (lastSet === null) return "";
    const counts = new Map<string, number>();
    for (const entry of lastSet) {
      const name = workspace.projectName(entry.projectId);
      counts.set(name, (counts.get(name) ?? 0) + 1);
    }
    const projects = [...counts].map(([name, count]) => (count > 1 ? `${name} ×${count}` : name)).join(", ");
    return `${projects} · ${lastSet.length} new ${lastSet.length === 1 ? "process" : "processes"}`;
  });

  function move(step: number): void {
    const index = entries.findIndex((entry) => entry.key === cursor);
    const next = entries[Math.max(0, Math.min(entries.length - 1, index + step))];
    if (next !== undefined) chosen = next.key;
  }

  function setScope(projectId: string | null): void {
    scope = projectId;
    chosen = null;
    if (projectId !== null) launchLine?.setProject(projectId);
    input?.focus();
  }

  function resume(conversation: Conversation): void {
    if (conversation.cwdMissing) return;
    if (conversation.maybeOpenElsewhere) {
      guard = conversation;
      return;
    }
    void run("Resume", () => itemResume(conversation.tool, conversation.id, conversation.path));
  }

  function switchTo(item: ItemState): void {
    void run("Switch", () => itemSelect(item.id));
  }

  function activate(entry: Entry | null): void {
    if (entry === null) return;
    chosen = entry.key;
    switch (entry.kind) {
      case "launch":
        void launchLine?.start();
        break;
      case "lastset":
        void run("Resume last set", () => lastSetResume());
        break;
      case "lastset-dismiss":
        chosen = null;
        void run("Dismiss", () => lastSetDismiss());
        break;
      case "run":
        switchTo(entry.item);
        break;
      case "conv":
        resume(entry.conversation);
        break;
    }
  }

  /** Rows are options of the listbox; one handler activates whichever was clicked. */
  function onoptionclick(event: MouseEvent): void {
    const key = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-entry]")?.dataset.entry;
    if (key !== undefined) {
      input?.focus();
      activate(entryByKey.get(key) ?? null);
    }
  }

  function close(): void {
    if (variant === "sheet") {
      void run("Close", () => sheetClose());
    } else if (query !== "" || scope !== null) {
      query = "";
      scope = null;
      chosen = null;
    }
  }

  function oninputkeydown(event: KeyboardEvent): void {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    switch (event.key) {
      case "ArrowDown":
        move(1);
        break;
      case "ArrowUp":
        move(-1);
        break;
      case "Enter":
        activate(cursorEntry);
        break;
      case "Backspace":
        if (query !== "" || scope === null) return;
        setScope(null);
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  function onrootkeydown(event: KeyboardEvent): void {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey) {
      event.preventDefault();
      chosen = "launch";
      void launchLine?.start();
    } else if (event.key === "Escape" && !event.defaultPrevented) {
      event.preventDefault();
      if (guard !== null) {
        guard = null;
        input?.focus();
      } else {
        close();
      }
    }
  }

  // Esc still closes when focus left the palette (e.g. clicked chrome); the sidebar handles its own.
  function onwindowkeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || event.defaultPrevented || root === null) return;
    if (root.contains(document.activeElement)) return;
    onrootkeydown(event);
  }

  /** A secondary-line segment; the model renders with its 10 px provider logo (unless the tool implies it). */
  type SubPart = string | { model: string; tool: string };

  function subText(parts: SubPart[]): string {
    return parts.map((part) => (typeof part === "string" ? part : modelShort(part.model))).join(" · ");
  }

  function savedSub(conversation: Conversation): SubPart[] {
    if (conversation.cwdMissing) return [`folder missing: ${tildePath(conversation.cwd, home)}`];
    if (conversation.maybeOpenElsewhere) {
      return [
        `started ${calendarDay(conversation.createdAt, clock.now)}`,
        `last write ${spokenAge(conversation.updatedAt, clock.now)} ago`,
      ];
    }
    const tool = workspace.toolLabel(conversation.tool);
    const model = conversation.model === null ? [] : [{ model: conversation.model, tool: conversation.tool }];
    if (scope !== null) return [tool, ...model, `started ${calendarDay(conversation.createdAt, clock.now)}`];
    return [workspace.projectName(conversation.projectId), tool, ...model];
  }

  function savedStatus(conversation: Conversation): string {
    if (conversation.cwdMissing) return "can't resume";
    if (conversation.maybeOpenElsewhere) return "may be open elsewhere";
    return recency(conversation.updatedAt, clock.now);
  }

  /** Board 11: project · tool · model; the state word only when no pill already shows it. */
  function runningSub(item: ItemState, stateText: string | null): SubPart[] {
    const parts: SubPart[] = [];
    if (scope === null) parts.push(workspace.projectName(item.projectId));
    parts.push(workspace.toolLabel(item.tool));
    if (stateText !== null) parts.push(stateText);
    if (item.model !== null) parts.push({ model: item.model, tool: item.tool });
    // Board 28: an agent on job lists its job labels, oldest first.
    if (item.activity === "on-job") parts.push(item.backgroundJobs.map((job) => job.label).join(", "));
    if (item.id === workspace.previousItemId) parts.push("previous item");
    return parts;
  }

  function runningAct(item: ItemState): string {
    return item.id === active?.id && workspace.state?.view === "sheet" ? "back" : "switch";
  }
  const separator = " · ";
</script>

{#snippet subLine(parts: SubPart[])}
  {#each parts as part, index (index)}{#if index > 0}{separator}{/if}{#if typeof part === "string"}{part}{:else}<ProviderMark
        model={part.model}
        tool={part.tool}
        inline
      />{modelShort(part.model)}{/if}{/each}
{/snippet}

<svelte:window onkeydown={onwindowkeydown} />

<div
  class="sheet wide palette"
  class:launcher={variant === "launcher"}
  bind:this={root}
  bind:clientWidth={width}
  onkeydown={onrootkeydown}
  role="dialog"
  aria-label={title}
  tabindex="-1"
>
  <div class="card">
    <div class="head">
      <h2>{title}</h2>
      {#if variant === "sheet" && active !== null}
        <button type="button" class="btn sm ghost back" onclick={close}>Back to terminal<kbd>Esc</kbd></button>
      {/if}
    </div>

    {#if workspace.state?.hostError}
      <div class="banner" role="alert">
        <Icon name="alert" color="#ff5370" />
        <span class="grow"
          ><b>The terminal host failed to start.</b><span class="muted selectable">{workspace.state.hostError}</span
          ><span class="muted">Launches will fail until Ultra 9000 restarts.</span></span
        >
      </div>
    {/if}

    <label class="pin">
      <Icon name="search" />
      {#if scope !== null}
        <button
          type="button"
          class="chip"
          tabindex="-1"
          aria-label="Remove project filter {scopeName}"
          onclick={() => setScope(null)}
        >
          {scopeName}<Icon name="close" small />
        </button>
      {/if}
      <input
        bind:this={input}
        bind:value={query}
        data-sheet-autofocus
        spellcheck="false"
        autocomplete="off"
        role="combobox"
        aria-label="Search running items and saved conversations"
        aria-autocomplete="list"
        aria-expanded={guard === null}
        aria-controls={listId}
        aria-activedescendant={guard === null ? optionId(cursor) : undefined}
        placeholder={scope === null
          ? "Search conversations and projects"
          : `Search ${scopeName}, or press Backspace to search everything`}
        oninput={() => (chosen = null)}
        onkeydown={oninputkeydown}
      />
      {#if q !== "" && !searching}
        <span class="count">{saved.length} of {savedTotal}</span>
      {/if}
    </label>

    {#if guard !== null}
      {@const target = guard}
      <div class="guard on-panel" role="alertdialog" aria-labelledby="guard-title">
        <div class="guard-title">
          <Icon name="warning" color="#ffcb6b" />
          <h2 id="guard-title">“{target.title}” may be open elsewhere</h2>
        </div>
        <p>
          Its session file was written <b>{spokenAge(target.updatedAt, clock.now)} ago</b>, but it isn't running in
          Ultra 9000. If
          {workspace.toolLabel(target.tool)} has it open in another terminal, resuming here too can interleave both conversations.
        </p>
        <div class="acts">
          <button type="button" class="btn" bind:this={guardCancel} onclick={() => ((guard = null), input?.focus())}
            >Cancel<kbd>Esc</kbd></button
          >
          <button type="button" class="btn" onclick={() => run("Start", () => itemLaunch(target.tool, target.cwd))}
            >Start new instead</button
          >
          <button
            type="button"
            class="btn danger"
            onclick={() => run("Resume", () => itemResume(target.tool, target.id, target.path))}>Resume anyway</button
          >
        </div>
      </div>
    {:else}
      <div class="body" class:wide={width >= WIDE}>
        <div class="main">
          <LaunchLine
            bind:this={launchLine}
            initialProjectId={scope ??
              active?.projectId ??
              workspace.lastProjectId ??
              workspace.state?.projects[0]?.id ??
              null}
            selected={cursor === "launch"}
            onactivate={() => (chosen = "launch")}
          />
          <div class="results scroll on-panel" bind:this={results}>
            {#if conversations.error !== null}
              <div class="banner" role="alert">
                <span class="grow"
                  ><span class="selectable">{conversations.error}</span><span class="acts"
                    ><button type="button" class="btn sm" onclick={() => conversations.refresh()}>Retry</button></span
                  ></span
                >
              </div>
            {/if}
            {#if search.error !== null && search.key === searchKey}
              <div class="banner" role="alert"><span class="grow selectable">{search.error}</span></div>
            {/if}
            {#each conversations.list?.errors ?? [] as storeError, index (index)}
              <div class="banner warn" role="alert">
                <span class="grow">
                  <b>Couldn't read {workspace.toolLabel(storeError.tool)}'s conversation store</b>
                  <span class="muted selectable">{storeError.message}. Running items are unaffected.</span>
                  {#if showStorePaths}<code class="selectable">{storeError.path}</code>{/if}
                  <span class="acts">
                    <button type="button" class="btn sm" onclick={() => conversations.refresh()}>Retry</button>
                    <button type="button" class="btn sm ghost" onclick={() => (showStorePaths = !showStorePaths)}
                      >{showStorePaths ? "Hide path" : "Show path"}</button
                    >
                  </span>
                </span>
              </div>
            {/each}

            <!-- Clicks keep focus in the search input; section headers are visual (groups carry the names). -->
            <div
              class="options"
              id={listId}
              role="listbox"
              tabindex="-1"
              aria-label="Start, switch or resume"
              aria-busy={searching || (conversations.list?.scanning ?? false)}
              onmousedown={(event) => event.preventDefault()}
              onclick={onoptionclick}
              onkeydown={oninputkeydown}
            >
              <div class="visually-hidden" role="option" id={optionId("launch")} aria-selected={cursor === "launch"}>
                {launchName}
              </div>

              {#if lastSet !== null && lastSet.length > 0}
                <div class="lastset" role="group" aria-label="Last quit">
                  <div
                    class="pr"
                    class:sel={cursor === "lastset"}
                    role="option"
                    id={optionId("lastset")}
                    data-entry="lastset"
                    aria-selected={cursor === "lastset"}
                    aria-label="Resume the {lastSet.length} {lastSet.length === 1
                      ? 'agent'
                      : 'agents'} from your last quit, {lastSetSummary}"
                  >
                    <span class="tile resume-tile"><Icon name="resume" /></span>
                    <span class="tx">
                      <span class="t"
                        >Resume the {lastSet.length}
                        {lastSet.length === 1 ? "agent" : "agents"} from your last quit</span
                      >
                      <span class="s">{lastSetSummary}</span>
                    </span>
                    <span class="act"
                      >{#if cursor === "lastset"}<kbd>↵</kbd>{/if}resume all</span
                    >
                  </div>
                  <div
                    class="btn sm ghost dismiss"
                    class:sel={cursor === "lastset-dismiss"}
                    role="option"
                    id={optionId("lastset-dismiss")}
                    data-entry="lastset-dismiss"
                    aria-selected={cursor === "lastset-dismiss"}
                    aria-label="Dismiss the last quit"
                  >
                    Dismiss
                  </div>
                </div>
              {/if}

              {#if running.length > 0}
                <div class="sec" aria-hidden="true">
                  Running{scope === null ? "" : ` in ${scopeName}`}<span class="r">↵ switches</span>
                </div>
                <div class="group" role="group" aria-label="Running{scope === null ? '' : ` in ${scopeName}`}">
                  {#each running as item (item.id)}
                    {@const view = itemView(item, { tool: workspace.tool(item.tool), now: clock.now, home })}
                    {@const key = `run:${item.id}`}
                    {@const dis = workspace.disambiguation.get(item.id) ?? ""}
                    {@const sub = runningSub(item, view.pill === "" ? view.stateText : null)}
                    <div
                      class="pr"
                      class:sel={cursor === key}
                      role="option"
                      id={optionId(key)}
                      data-entry={key}
                      aria-selected={cursor === key}
                      aria-label="{view.title}{dis}, {subText(sub)}, {runningAct(item)}"
                    >
                      <ToolTile
                        tool={item.tool}
                        model={item.model}
                        badge={view.badge}
                        badgeGlyph={view.badgeGlyph}
                        unread={item.unread}
                      />
                      <span class="tx">
                        <span class="t"
                          >{#each highlight(view.title, q) as part, index (index)}{#if part.match}<mark
                                >{part.text}</mark
                              >{:else}{part.text}{/if}{/each}<span class="dis">{dis}</span></span
                        >
                        <span class="s" title={view.pillClass === "wait" ? view.tooltip : undefined}
                          >{@render subLine(sub)}</span
                        >
                      </span>
                      {#if view.pill !== ""}<span class="pill {view.pillClass}">{view.pill}</span>{/if}
                      <span class="act"
                        >{#if cursor === key}<kbd>↵</kbd>{/if}{runningAct(item)}</span
                      >
                    </div>
                  {/each}
                </div>
              {/if}

              {#if scope !== null || q !== ""}
                <div class="sec" aria-hidden="true">
                  {savedLabel}{q === "" ? ` · ${savedTotal}` : ""}
                  <span class="r"
                    >{conversations.list?.scanning
                      ? "scanning…"
                      : searching
                        ? "searching…"
                        : "↵ resumes as a new process"}</span
                  >
                </div>
              {:else if sections.length > 0 || conversations.list?.scanning}
                <div class="sec" aria-hidden="true">
                  {savedLabel}
                  <span class="r"
                    >{conversations.list?.scanning ? "scanning…" : `all projects · ${savedTotal} saved`}</span
                  >
                </div>
              {/if}

              {#each sections as section (section.title)}
                {#if section.title !== ""}
                  <div class="sub" aria-hidden="true">{section.title}</div>
                {/if}
                <div
                  class="group"
                  role="group"
                  aria-label={section.title === "" ? savedLabel : `${savedLabel}, ${section.title}`}
                >
                  {#each section.rows as conversation (conversation.path)}
                    {@const key = `conv:${conversation.path}`}
                    {@const status = savedStatus(conversation)}
                    <div
                      class="pr"
                      class:sel={cursor === key}
                      class:dis={conversation.cwdMissing}
                      role="option"
                      id={optionId(key)}
                      data-entry={key}
                      aria-selected={cursor === key}
                      aria-disabled={conversation.cwdMissing}
                      aria-label="{conversation.title}{savedDis.get(conversation.path) ?? ''}, {subText(
                        savedSub(conversation),
                      )}, {status}"
                    >
                      <ToolTile tool={conversation.tool} model={conversation.model} colour="picker" />
                      <span class="tx">
                        <span class="t"
                          >{#each highlight(conversation.title, q) as part, index (index)}{#if part.match}<mark
                                >{part.text}</mark
                              >{:else}{part.text}{/if}{/each}<span class="dis"
                            >{savedDis.get(conversation.path) ?? ""}</span
                          ></span
                        >
                        <span class="s">{@render subLine(savedSub(conversation))}</span>
                      </span>
                      {#if conversation.maybeOpenElsewhere && !conversation.cwdMissing}
                        <span class="pill warn">{status}</span>
                      {:else}
                        <span class="act"
                          >{#if cursor === key && !conversation.cwdMissing}<kbd>↵</kbd> resume{:else}{status}{/if}</span
                        >
                      {/if}
                    </div>
                  {/each}
                </div>
              {/each}
            </div>

            {#if conversations.list === null && conversations.error === null}
              <div class="empty">Reading saved conversations…</div>
            {:else if !searching && saved.length === 0 && !(conversations.list?.scanning ?? false)}
              {#if q !== ""}
                <div class="empty">
                  {running.length === 0 ? `Nothing matches “${q}”.` : "No saved conversations match."}
                </div>
              {:else if scope !== null}
                <div class="empty centered">
                  <span class="hi">No saved conversations in {scopeName} yet</span>
                  <button type="button" class="btn pri sm" onclick={() => activate(entryByKey.get("launch") ?? null)}
                    >Start {launchTool === "" ? "" : `${launchTool} `}in {scopeName}<kbd>↵</kbd></button
                  >
                </div>
              {:else}
                <div class="empty">No saved conversations yet. Start something new above.</div>
              {/if}
            {/if}
          </div>
        </div>
        {#if width >= WIDE}
          <aside class="side">
            {#if cursorEntry?.kind === "conv"}
              <PaletteDetail
                conversation={cursorEntry.conversation}
                item={null}
                onresume={resume}
                onswitch={switchTo}
              />
            {:else if cursorEntry?.kind === "run"}
              <PaletteDetail conversation={null} item={cursorEntry.item} onresume={resume} onswitch={switchTo} />
            {:else}
              <div class="sec">Projects</div>
              {#each conversations.projects as project (project.projectId)}
                <button
                  type="button"
                  class="pj"
                  class:on={scope === project.projectId}
                  onclick={() => setScope(project.projectId)}
                >
                  <span class="n">{workspace.projectName(project.projectId)}</span><span class="c">{project.count}</span
                  >
                </button>
              {/each}
            {/if}
          </aside>
        {/if}
      </div>
    {/if}

    <div class="foot">
      <span><kbd>↑</kbd><kbd>↓</kbd> move</span>
      <span><kbd>↵</kbd> start / resume / switch</span>
      <span><kbd>Ctrl+↵</kbd> start new</span>
      <span><kbd>Tab</kbd> tool, project, folder</span>
      {#if scope !== null}<span><kbd>Backspace</kbd> on empty: all projects</span>{/if}
      {#if workspace.shortcut("new-terminal") !== null}<span
          ><kbd>{workspace.shortcut("new-terminal")}</kbd> terminal here</span
        >{/if}
      {#if variant === "sheet"}<span><kbd>Esc</kbd> {active === null ? "close" : "back to terminal"}</span>{/if}
    </div>
  </div>
  {#if variant === "sheet" && active !== null}
    <div class="hidden-note note">
      <Icon name="panel" small />Terminal hidden while open · Esc returns to “{active.title}”
    </div>
  {/if}
</div>

<style>
  .palette {
    padding-top: 28px;
    padding-bottom: 30px;
  }
  .palette.launcher {
    padding-top: 36px;
  }
  .card {
    flex: 1;
    gap: 10px;
  }
  /* Fixed to the h2 line: the baseline-aligned Back button would otherwise make it fractional and
     shift every result tile off the pixel grid. */
  .head {
    display: flex;
    align-items: baseline;
    gap: 12px;
    height: 26px;
    flex: none;
  }
  .back {
    margin-left: auto;
  }
  .pin {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 42px;
    padding: 0 12px;
    border-radius: 9px;
    background: var(--well);
    border: 1px solid #3a3a3a;
    font-size: 15px;
    color: var(--hi);
    flex: none;
  }
  .pin:focus-within {
    border-color: var(--blue);
    box-shadow: 0 0 0 1px var(--blue);
  }
  .pin input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--hi);
    font-size: 15px;
    outline: none;
  }
  .pin input::placeholder {
    color: #6b6b6b;
  }
  .count {
    font-size: 12px;
    color: var(--dim);
    flex: none;
  }
  .body {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 18px;
    min-height: 0;
    flex: 1;
  }
  .body.wide {
    grid-template-columns: minmax(0, 1fr) 300px;
  }
  .main {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
    min-height: 0;
  }
  .results {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-height: 0;
    flex: 1;
    padding-bottom: 6px;
  }
  .results .banner {
    margin: 4px 0;
  }
  /* The listbox and its groups keep the results' single column. */
  .options,
  .group {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: none;
  }
  .options {
    position: relative;
  }
  .sub {
    display: flex;
    gap: 8px;
    font-size: 11px;
    line-height: 15px;
    color: #6d6d6d;
    padding: 8px 4px 2px;
    flex: none;
  }
  .lastset {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    flex: none;
  }
  .lastset .pr {
    flex: 1;
    width: auto;
  }
  .lastset .dismiss.sel {
    background: var(--sel);
    color: var(--hi);
  }
  /* The 14 px resume icon at the tile's integer padding (5 px), like a 14 px mark box. */
  .resume-tile {
    display: grid;
    place-items: center;
    color: var(--text);
  }
  .empty {
    padding: 10px;
    color: var(--dim);
    flex: none;
  }
  .empty.centered {
    padding: 18px 10px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }
  .empty .hi {
    color: var(--hi);
  }
  .side {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    min-height: 0;
    overflow-y: auto;
  }
  .pj {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 8px;
    border-radius: 7px;
    font-size: 13px;
    color: var(--hi);
    width: 100%;
    flex: none;
  }
  .pj:hover,
  .pj.on {
    background: var(--hover);
  }
  .pj .n {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pj .c {
    color: var(--dim);
    font-size: 12px;
  }
  .guard {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 22px 26px;
    border-radius: 10px;
    border: 1px solid #333;
    background: var(--panel);
  }
  .guard-title {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .guard-title h2 {
    font-size: 18px;
  }
  .guard p {
    font-size: 13.5px;
  }
  .guard b {
    color: var(--hi);
  }
  .guard .acts {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  /* Small tiles: the palette takes the whole content area and sections shrink to the best matches. */
  @media (max-width: 760px), (max-height: 520px) {
    .palette,
    .palette.launcher {
      padding: 12px 12px 8px;
    }
    .card {
      gap: 8px;
    }
    .head,
    .foot,
    .note {
      display: none;
    }
    .pin {
      height: 34px;
      font-size: 13.5px;
    }
    .pin input {
      font-size: 13.5px;
    }
  }
</style>
