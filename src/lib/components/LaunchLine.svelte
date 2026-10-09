<script lang="ts">
  // The palette's "New <tool> in <project> <folder>" line. Defaults are visible before anything
  // starts; the folder is validated as data (path_check) while typing and Enter cannot launch into
  // a folder that doesn't exist. Tab walks tool → project → folder; menus open with ↓.
  import { untrack } from "svelte";
  import { conversations } from "../conversations.svelte";
  import { run } from "../errors.svelte";
  import { tildePath } from "../format";
  import { errorMessage, itemLaunch, pathCheck, pickDirectory, type PathCheck, type ToolInfo } from "../native";
  import { projectToolKey, settings } from "../settings.svelte";
  import { workspace } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import ToolTile from "./ToolTile.svelte";

  let {
    initialProjectId,
    selected,
    onactivate,
  }: {
    initialProjectId: string | null;
    /** The palette cursor is on this line (Enter starts). */
    selected: boolean;
    /** The line was interacted with; the palette moves its cursor here. */
    onactivate: () => void;
  } = $props();

  interface ProjectChoice {
    id: string;
    name: string;
    path: string;
  }

  type MenuEntry =
    | { kind: "tool"; tool: ToolInfo }
    | { kind: "project"; project: ProjectChoice }
    | { kind: "other-folder" }
    | { kind: "browse" }
    | { kind: "separator" };

  const home = $derived(workspace.state?.defaultCwd ?? "");

  const projectChoices: ProjectChoice[] = $derived.by(() => {
    const choices = new Map<string, ProjectChoice>();
    for (const project of workspace.state?.projects ?? []) {
      choices.set(project.id, { id: project.id, name: project.name, path: project.path });
    }
    for (const saved of conversations.projects) {
      if (!choices.has(saved.projectId)) {
        choices.set(saved.projectId, {
          id: saved.projectId,
          name: workspace.projectName(saved.projectId),
          path: saved.projectId,
        });
      }
    }
    return [...choices.values()];
  });

  function projectPath(projectId: string | null): string {
    if (projectId === null) return home;
    return workspace.projectsById.get(projectId)?.path ?? projectId;
  }

  /** Project override, else launch.default_tool, else OMP (or the first agent tool). */
  function defaultTool(projectId: string | null): string {
    const tools = workspace.state?.tools ?? [];
    const candidates = [
      projectId === null ? undefined : workspace.projectsById.get(projectId)?.defaultTool,
      projectId === null ? undefined : settings.values.get(projectToolKey(projectPath(projectId)))?.value,
      settings.values.get("launch.default_tool")?.value,
      "omp",
    ];
    for (const candidate of candidates) {
      if (typeof candidate === "string" && tools.some((tool) => tool.id === candidate)) return candidate;
    }
    return tools.find((tool) => tool.kind === "agent")?.id ?? tools[0]?.id ?? "omp";
  }

  // The palette mounts only with a snapshot, so defaults resolve immediately; later, the user's
  // choices win. Until a tool is chosen explicitly, it follows the launch project's default.
  let projectId = $state<string | null>(untrack(() => initialProjectId));
  let chosenTool = $state<string | null>(null);
  let folder = $state(untrack(() => projectPath(initialProjectId)));
  let check = $state<{ path: string; result: PathCheck | null; error: string | null }>({
    path: "",
    result: null,
    error: null,
  });
  let starting = $state(false);
  let launchError = $state<string | null>(null);
  let menu = $state<"tool" | "project" | null>(null);
  let menuIndex = $state(0);
  let folderInput = $state<HTMLInputElement | null>(null);
  let toolAnchor = $state<HTMLElement | null>(null);
  let projectAnchor = $state<HTMLElement | null>(null);

  const toolId = $derived(chosenTool ?? defaultTool(projectId));
  const tool = $derived(workspace.tool(toolId));
  const toolLabel = $derived(tool?.label ?? toolId);
  const trimmed = $derived(folder.trim());
  const current = $derived(check.path === trimmed ? check.result : null);
  const valid = $derived(current?.normalized != null);
  const canStart = $derived(valid && tool !== undefined && tool.available !== false && !starting);
  const projectLabel = $derived.by(() => {
    if (projectId === null) return trimmed === home ? "home" : "folder";
    const path = projectPath(projectId);
    const inside = trimmed === path || trimmed.startsWith(`${path}/`);
    return inside ? workspace.projectName(projectId) : "another folder";
  });

  // Validate the folder as data while typing (never through a shell).
  $effect(() => {
    const path = trimmed;
    if (path === "") {
      check = { path, result: null, error: null };
      return;
    }
    const timer = setTimeout(() => {
      pathCheck(path).then(
        (result) => {
          if (folder.trim() === path) check = { path, result, error: null };
        },
        (error) => {
          if (folder.trim() === path) check = { path, result: null, error: errorMessage(error) };
        },
      );
    }, 150);
    return () => clearTimeout(timer);
  });

  const menuEntries: MenuEntry[] = $derived.by(() => {
    if (menu === "tool") {
      const tools = workspace.state?.tools ?? [];
      const agents = tools
        .filter((entry) => entry.kind === "agent")
        .map((entry): MenuEntry => ({ kind: "tool", tool: entry }));
      const terminals = tools
        .filter((entry) => entry.kind === "terminal")
        .map((entry): MenuEntry => ({ kind: "tool", tool: entry }));
      return agents.length > 0 && terminals.length > 0
        ? [...agents, { kind: "separator" }, ...terminals]
        : [...agents, ...terminals];
    }
    if (menu === "project") {
      const projects = projectChoices.map((project): MenuEntry => ({ kind: "project", project }));
      return [
        ...projects,
        ...(projects.length > 0 ? [{ kind: "separator" } as const] : []),
        { kind: "other-folder" },
        { kind: "browse" },
      ];
    }
    return [];
  });

  function selectable(entry: MenuEntry): boolean {
    return entry.kind === "tool" ? entry.tool.available !== false : entry.kind !== "separator";
  }

  function openMenu(which: "tool" | "project"): void {
    onactivate();
    menu = which;
    const index = menuEntries.findIndex((entry) =>
      which === "tool"
        ? entry.kind === "tool" && entry.tool.id === toolId
        : entry.kind === "project" && entry.project.id === projectId,
    );
    menuIndex = Math.max(0, index);
  }

  function moveMenu(step: number): void {
    const count = menuEntries.length;
    if (count === 0) return;
    let next = menuIndex;
    for (let tries = 0; tries < count; tries += 1) {
      next = (next + step + count) % count;
      if (menuEntries[next] !== undefined && menuEntries[next].kind !== "separator") break;
    }
    menuIndex = next;
  }

  function choose(entry: MenuEntry | undefined): void {
    if (entry === undefined || !selectable(entry)) return;
    menu = null;
    switch (entry.kind) {
      case "tool":
        chosenTool = entry.tool.id;
        break;
      case "project":
        setProject(entry.project.id);
        break;
      case "other-folder":
        projectId = null;
        focusFolder();
        break;
      case "browse":
        void browse();
        break;
      case "separator":
        break;
    }
  }

  function onpickkeydown(event: KeyboardEvent, which: "tool" | "project"): void {
    if (menu !== which) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        event.stopPropagation();
        openMenu(which);
      }
      return;
    }
    switch (event.key) {
      case "ArrowDown":
        moveMenu(1);
        break;
      case "ArrowUp":
        moveMenu(-1);
        break;
      case "Enter":
      case " ":
        choose(menuEntries[menuIndex]);
        break;
      case "Escape":
        menu = null;
        break;
      case "Tab":
        menu = null;
        return;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  }

  function onwindowpointerdown(event: PointerEvent): void {
    if (menu === null) return;
    const anchor = menu === "tool" ? toolAnchor : projectAnchor;
    if (anchor !== null && !anchor.contains(event.target as Node)) menu = null;
  }

  async function browse(): Promise<void> {
    onactivate();
    const picked = await run("Choose folder", () =>
      pickDirectory(valid && current?.normalized ? current.normalized : home),
    );
    if (typeof picked === "string") {
      folder = picked;
      projectId =
        projectChoices.find((project) => picked === project.path || picked.startsWith(`${project.path}/`))?.id ?? null;
    }
  }

  /** The launch project (from the projects column or the project menu): folder and tool follow. */
  export function setProject(id: string): void {
    projectId = id;
    folder = projectPath(id);
    launchError = null;
  }

  export function focusFolder(): void {
    onactivate();
    folderInput?.focus();
    folderInput?.select();
  }

  /** The tool Enter starts (for the palette's accessible launch option and empty-project button). */
  export function toolName(): string {
    return toolLabel;
  }

  /** Where Enter starts it: the project name, "home", "folder" or "another folder". */
  export function targetName(): string {
    return projectLabel;
  }

  /** Starts the chosen tool; an invalid folder takes focus instead of launching. */
  export async function start(): Promise<void> {
    if (starting) return;
    const cwd = current?.normalized;
    if (!canStart || cwd == null) {
      if (!valid) focusFolder();
      return;
    }
    starting = true;
    launchError = null;
    try {
      // Native creates the row at once, selects it and closes the sheet.
      await itemLaunch(toolId, cwd);
    } catch (error) {
      launchError = `Couldn't start ${toolLabel}: ${errorMessage(error)}`;
    } finally {
      starting = false;
    }
  }
</script>

<svelte:window onpointerdown={onwindowpointerdown} />

<div class="launch" class:sel={selected}>
  <span class="lbl">New</span>
  <div class="anchor" bind:this={toolAnchor}>
    <button
      type="button"
      class="pick"
      aria-haspopup="listbox"
      aria-expanded={menu === "tool"}
      aria-label="Tool: {toolLabel}"
      onclick={() => (menu === "tool" ? (menu = null) : openMenu("tool"))}
      onkeydown={(event) => onpickkeydown(event, "tool")}
    >
      <ToolTile tool={toolId} size={16} colour="launch" />{toolLabel}<Icon name="chevron-down" small />
    </button>
    {#if menu === "tool"}
      <div class="menu" role="listbox" aria-label="Tool">
        {#each menuEntries as entry, index (index)}
          {#if entry.kind === "separator"}
            <div class="hr"></div>
          {:else if entry.kind === "tool"}
            {@const option = entry.tool}
            <button
              type="button"
              class="mi"
              class:sel={index === menuIndex}
              class:dis={option.available === false}
              role="option"
              aria-selected={option.id === toolId}
              aria-disabled={option.available === false}
              tabindex="-1"
              onclick={() => choose(entry)}
              onmouseenter={() => (menuIndex = index)}
            >
              <span class="tool-choice" class:faded={option.available === false}
                ><ToolTile tool={option.id} colour="launch" /></span
              >{option.label}
              <span class="s" class:warn={option.available === false}>
                {#if option.available === false}
                  {option.unavailableReason ?? "unavailable"}
                {:else if option.available === null}
                  checking…
                {:else if option.id === defaultTool(projectId)}
                  default{projectId === null ? "" : ` for ${workspace.projectName(projectId)}`} · ✓
                {:else if option.id === "shell" && workspace.shortcut("new-terminal") !== null}
                  {workspace.shortcut("new-terminal")}
                {:else if option.id === "neovim" && workspace.shortcut("new-neovim") !== null}
                  {workspace.shortcut("new-neovim")}
                {:else}
                  {option.path === null ? "" : tildePath(option.path, home)}
                {/if}
              </span>
            </button>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
  <span class="lbl">in</span>
  <div class="anchor" bind:this={projectAnchor}>
    <button
      type="button"
      class="pick"
      aria-haspopup="listbox"
      aria-expanded={menu === "project"}
      aria-label="Project: {projectLabel}"
      onclick={() => (menu === "project" ? (menu = null) : openMenu("project"))}
      onkeydown={(event) => onpickkeydown(event, "project")}
    >
      {projectLabel}<Icon name="chevron-down" small />
    </button>
    {#if menu === "project"}
      <div class="menu" role="listbox" aria-label="Project">
        {#each menuEntries as entry, index (index)}
          {#if entry.kind === "separator"}
            <div class="hr"></div>
          {:else if entry.kind === "project"}
            <button
              type="button"
              class="mi"
              class:sel={index === menuIndex}
              role="option"
              aria-selected={entry.project.id === projectId}
              tabindex="-1"
              onclick={() => choose(entry)}
              onmouseenter={() => (menuIndex = index)}
            >
              {entry.project.name}<span class="s">{tildePath(entry.project.path, home)}</span>
            </button>
          {:else}
            <button
              type="button"
              class="mi"
              class:sel={index === menuIndex}
              role="option"
              aria-selected="false"
              tabindex="-1"
              onclick={() => choose(entry)}
              onmouseenter={() => (menuIndex = index)}
            >
              <Icon name="folder" />{entry.kind === "browse" ? "Choose folder…" : "Another folder…"}
            </button>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
  <label class="field folder" class:err={current !== null && current.error !== null}>
    <Icon name="folder" small />
    <input
      bind:this={folderInput}
      bind:value={folder}
      class="mono"
      spellcheck="false"
      autocomplete="off"
      aria-label="Folder"
      aria-invalid={current !== null && current.error !== null}
      onfocus={onactivate}
      oninput={() => (launchError = null)}
      onkeydown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          event.stopPropagation();
          void start();
        }
      }}
    />
    <button type="button" class="iconbtn" aria-label="Choose folder…" title="Choose folder…" onclick={browse}>
      <Icon name="search" small />
    </button>
  </label>
  <span class="go">
    <button type="button" class="btn sm pri" disabled={!canStart} onclick={() => void start()}>
      {starting ? "Starting…" : `Start ${toolLabel}`}<kbd>{selected ? "↵" : "Ctrl+↵"}</kbd>
    </button>
  </span>
</div>
{#if current !== null && current.error !== null}
  <div class="hint err" role="alert">
    {current.error}{#if current.suggestion !== null}
      Did you mean <button type="button" class="link" onclick={() => (folder = current.suggestion ?? folder)}
        >{current.suggestion}</button
      >?{/if}
    <span class="dim"><kbd>↵</kbd> is disabled until the folder exists.</span>
  </div>
{:else if check.error !== null && check.path === trimmed}
  <div class="hint err" role="alert">Couldn't check the folder: {check.error}</div>
{:else if tool !== undefined && tool.available === false}
  <div class="hint warn">{toolLabel}: {tool.unavailableReason ?? "unavailable"}</div>
{/if}
{#if launchError !== null}
  <div class="hint err selectable" role="alert">{launchError}</div>
{/if}

<style>
  .launch {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    padding: 10px 12px;
    border-radius: 9px;
    border: 1px solid var(--line);
    background: #1d1d1d;
    font-size: 13px;
    flex: none;
  }
  .launch.sel {
    border-color: #5a5a5a;
    background: var(--sel);
  }
  .lbl {
    color: var(--dim);
  }
  .anchor {
    position: relative;
  }
  .anchor .menu {
    top: calc(100% + 4px);
    left: 0;
  }
  .tool-choice {
    display: block;
    flex: none;
  }
  .faded {
    opacity: 0.5;
  }
  .menu .s.warn {
    color: var(--yellow);
  }
  .folder {
    height: 26px;
    flex: 1 1 200px;
    min-width: 160px;
    padding: 0 2px 0 8px;
    gap: 6px;
    color: var(--dim);
  }
  .folder input {
    font-size: 12px;
  }
  .go {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .hint {
    font-size: 12px;
    padding: 0 4px;
    flex: none;
    display: flex;
    flex-wrap: wrap;
    gap: 4px 6px;
    align-items: baseline;
  }
  .hint.err {
    color: var(--err-text);
  }
  .hint.warn {
    color: var(--yellow);
  }
  .hint .dim {
    color: var(--dim);
  }
  .link {
    color: var(--hi);
    text-decoration: underline;
    font-family: var(--mono);
  }
</style>
