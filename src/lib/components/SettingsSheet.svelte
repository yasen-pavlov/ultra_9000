<script lang="ts">
  // Settings (board 17): a few common keys over the files in ~/.config/ultra-9000. Every row names
  // its file and key and when it applies; broken hand edits show a banner while the last good
  // value keeps working. Expanded/collapsed is UI state (state.json), not a setting.
  import { run } from "../errors.svelte";
  import { basename, clockSeconds, tildePath } from "../format";
  import {
    settingsOpenFile,
    settingsReset,
    settingsReveal,
    sheetClose,
    uiSet,
    type SettingsError,
    type WriteTarget,
  } from "../native";
  import { projectToolKey, settings } from "../settings.svelte";
  import { workspace } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import NumberField from "./NumberField.svelte";
  import SettingRow from "./SettingRow.svelte";

  let { sidebarExpanded }: { sidebarExpanded: boolean } = $props();

  let done = $state<HTMLButtonElement | null>(null);

  const config = $derived(settings.state);
  const home = $derived(workspace.state?.defaultCwd ?? "");
  const agents = $derived((workspace.state?.tools ?? []).filter((tool) => tool.kind === "agent"));
  const machineFile = $derived(config?.files.find((file) => file.name.startsWith("machines/")) ?? null);
  const ultraFile = $derived(config?.files.find((file) => file.name === "ultra.toml") ?? null);
  const existing = $derived((config?.files ?? []).filter((file) => file.exists).map((file) => file.name));
  const currentProject = $derived.by(() => {
    const id = workspace.active?.projectId ?? workspace.lastProjectId;
    if (id === null) return null;
    const project = workspace.projectsById.get(id);
    return { id, name: workspace.projectName(id), path: project?.path ?? id };
  });
  const globalDefault = $derived(settings.values.get("launch.default_tool")?.value);
  const pinned = $derived.by(() => {
    const value = settings.values.get("projects.pinned")?.value;
    return Array.isArray(value) ? value : [];
  });

  function fileName(path: string): string {
    return config?.files.find((file) => file.path === path)?.name ?? basename(path);
  }

  /** Where "Remove this key" writes: the file named by the error. */
  function errorTarget(error: SettingsError): WriteTarget {
    if (machineFile !== null && error.file === machineFile.path) return "machine";
    const project = error.key?.match(/^projects\."((?:[^"\\]|\\.)*)"\./);
    if (project) return { project: JSON.parse(`"${project[1]}"`) as string };
    return "auto";
  }

  $effect(() => {
    done?.focus();
  });

  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    void run("Close", () => sheetClose());
  }
</script>

<svelte:window {onkeydown} />

<div class="sheet settings">
  <div class="card">
    <div class="head">
      <h2>Settings</h2>
      <span class="dim">A few common keys. Everything else lives in the files.</span>
      <button
        type="button"
        class="btn sm ghost done"
        bind:this={done}
        data-sheet-autofocus
        onclick={() => run("Close", () => sheetClose())}>Done<kbd>Esc</kbd></button
      >
    </div>

    {#if settings.error !== null}
      <div class="banner" role="alert">
        <span class="grow"
          ><span class="selectable">{settings.error}</span><span class="acts"
            ><button type="button" class="btn sm" onclick={() => settings.retry()}>Retry</button></span
          ></span
        >
      </div>
    {/if}

    {#if config !== null}
      <div class="files">
        <span class="key code selectable">{tildePath(config.configDir, home)}/</span>
        <span class="dim">{existing.length > 0 ? existing.join(" · ") : "no files yet: built-in defaults"}</span>
        <span class="grow"></span>
        {#if ultraFile !== null}
          <button
            type="button"
            class="btn sm"
            onclick={() => run("Open in Neovim", () => settingsOpenFile(ultraFile.path))}
          >
            {ultraFile.exists ? "Open in Neovim" : "Create with comments"}
          </button>
        {/if}
        <button type="button" class="btn sm ghost" onclick={() => run("Reveal folder", () => settingsReveal())}
          >Reveal folder</button
        >
      </div>
    {/if}

    <div class="rows scroll">
      {#each config?.errors ?? [] as error, index (index)}
        <div class="banner" role="alert">
          <Icon name="alert" color="#ff5370" />
          <span class="grow">
            <span
              ><b>{fileName(error.file)}{error.line === null ? "" : `, line ${error.line}`}:</b>
              {#if error.key !== null}<code>{error.key}</code> —{/if}
              {error.message}</span
            >
            <span class="muted">
              {error.lastGood === null
                ? "Using the built-in default."
                : `Still using the last good value (${error.lastGood}).`}
              All other keys reloaded at {clockSeconds(config?.reloadedAt ?? Date.now())}.
            </span>
            <span class="acts">
              <button
                type="button"
                class="btn sm"
                onclick={() => run("Open file", () => settingsOpenFile(error.file, error.line ?? undefined))}
              >
                {error.line === null ? "Open file" : `Open at line ${error.line}`}
              </button>
              {#if error.key !== null}
                {@const key = error.key}
                <button
                  type="button"
                  class="btn sm ghost"
                  onclick={() => run("Remove key", () => settingsReset(key, errorTarget(error)))}
                  >Remove this key</button
                >
              {/if}
            </span>
          </span>
        </div>
      {/each}

      <div class="sec">Launching</div>
      <SettingRow
        key="launch.default_tool"
        name="Default agent tool"
        description="Used by New agent and the launch line, unless a project sets its own."
        timing="next launch"
      >
        {#snippet control(entry, write, pending)}
          <span class="seg">
            {#each agents as tool (tool.id)}
              <button
                type="button"
                class:on={entry?.value === tool.id}
                disabled={pending}
                aria-pressed={entry?.value === tool.id}
                onclick={() => write(tool.id)}>{tool.label}</button
              >
            {/each}
          </span>
        {/snippet}
      </SettingRow>
      {#if currentProject !== null}
        {@const project = currentProject}
        <SettingRow
          key={projectToolKey(project.path)}
          name="{project.name} uses"
          description="Per-project override. Shown here because {project.name} is the current project."
          timing="next launch"
          project={project.path}
        >
          {#snippet control(entry, write, pending)}
            {@const inherited = workspace.toolLabel(typeof globalDefault === "string" ? globalDefault : "omp")}
            <span class="seg">
              <button
                type="button"
                class:on={entry?.source == null}
                disabled={pending}
                aria-pressed={entry?.source == null}
                onclick={() => {
                  if (entry?.source != null)
                    void run("Reset", () => settingsReset(projectToolKey(project.path), "auto"));
                }}>Default ({inherited})</button
              >
              {#each agents as tool (tool.id)}
                <button
                  type="button"
                  class:on={entry?.source != null && entry.value === tool.id}
                  disabled={pending}
                  aria-pressed={entry?.source != null && entry.value === tool.id}
                  onclick={() => write(tool.id)}>{tool.label}</button
                >
              {/each}
            </span>
          {/snippet}
        </SettingRow>
      {/if}
      <SettingRow
        key="icons.brand_color"
        name="Brand colour"
        description="Tool marks in their colours. Pickers keeps the sidebar and running items monochrome; Tool menu colours only the launch choices."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <span class="seg">
            <button
              type="button"
              class:on={entry?.value !== "pickers" && entry?.value !== "launch" && entry?.value !== "off"}
              disabled={pending}
              onclick={() => write("all")}>All</button
            >
            <button
              type="button"
              class:on={entry?.value === "pickers"}
              disabled={pending}
              onclick={() => write("pickers")}>Pickers</button
            >
            <button
              type="button"
              class:on={entry?.value === "launch"}
              disabled={pending}
              onclick={() => write("launch")}>Tool menu</button
            >
            <button type="button" class:on={entry?.value === "off"} disabled={pending} onclick={() => write("off")}
              >Off</button
            >
          </span>
        {/snippet}
      </SettingRow>

      <div class="sec">Sidebar</div>
      <SettingRow
        key="sidebar.density"
        name="Row density"
        description="Comfortable shows tool, model and age under each title."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <span class="seg">
            <button
              type="button"
              class:on={entry?.value !== "compact"}
              disabled={pending}
              onclick={() => write("comfortable")}>Comfortable</button
            >
            <button
              type="button"
              class:on={entry?.value === "compact"}
              disabled={pending}
              onclick={() => write("compact")}>Compact</button
            >
          </span>
        {/snippet}
      </SettingRow>
      <SettingRow
        key="sidebar.provider_mark"
        name="Provider mark"
        description="The model provider's logo on an agent's tile corner. Compact rows have no room for it; the context bar names the model."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <span class="seg">
            <button type="button" class:on={entry?.value !== "off"} disabled={pending} onclick={() => write("corner")}
              >Corner</button
            >
            <button type="button" class:on={entry?.value === "off"} disabled={pending} onclick={() => write("off")}
              >Off</button
            >
          </span>
        {/snippet}
      </SettingRow>
      <SettingRow
        key="layout.context_bar"
        name="Context bar"
        description="Above the terminal. Auto: only while the sidebar is collapsed. Never: a collapsed sidebar still shows it while a notice or error is open."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <span class="seg">
            {#each [["auto", "Auto"], ["always", "Always"], ["never", "Never"]] as [value, label] (value)}
              <button
                type="button"
                class:on={(entry?.value ?? "auto") === value}
                disabled={pending}
                onclick={() => write(value)}>{label}</button
              >
            {/each}
          </span>
        {/snippet}
      </SettingRow>
      <SettingRow key="sidebar.width" name="Sidebar width" description="Expanded width, 240–360 px." timing="live">
        {#snippet control(entry, write, pending)}
          <NumberField
            value={typeof entry?.value === "number" ? entry.value : undefined}
            unit="px"
            label="Sidebar width"
            min={240}
            max={360}
            disabled={pending}
            oncommit={write}
          />
        {/snippet}
      </SettingRow>
      <SettingRow
        key="sidebar.auto_collapse_below"
        name="Collapse automatically below"
        description="Window width under which the sidebar shows as the narrow strip."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <NumberField
            value={typeof entry?.value === "number" ? entry.value : undefined}
            unit="px"
            label="Collapse automatically below"
            min={0}
            max={10000}
            disabled={pending}
            oncommit={write}
          />
        {/snippet}
      </SettingRow>
      <div class="set ui">
        <span class="n">Sidebar</span>
        <span class="ctl">
          <button
            type="button"
            class="btn sm"
            onclick={() => run("Sidebar", () => uiSet({ sidebarExpanded: !sidebarExpanded }))}
          >
            {sidebarExpanded ? "Collapse" : "Expand"}
          </button>
        </span>
        <span class="d">Expanded or collapsed is remembered UI state (state.json), not a setting.</span>
        <span class="meta">not in a config file</span>
      </div>
      <SettingRow
        key="projects.pinned"
        name="Pinned projects"
        description="Stay in the sidebar with nothing running. A missing folder is shown as missing, not removed."
        timing="live"
      >
        {#snippet control(_entry, write, pending)}
          <span class="pins">
            {#each pinned as path, index (index)}
              <span class="chip">
                {tildePath(path, home)}
                <button
                  type="button"
                  class="unpin"
                  disabled={pending}
                  aria-label="Unpin {path}"
                  onclick={() => write(pinned.filter((entry) => entry !== path))}><Icon name="close" small /></button
                >
              </span>
            {/each}
            {#if currentProject !== null && !pinned.includes(currentProject.path)}
              {@const path = currentProject.path}
              <button type="button" class="btn xs" disabled={pending} onclick={() => write([...pinned, path])}
                >Pin {currentProject.name}</button
              >
            {/if}
          </span>
        {/snippet}
      </SettingRow>

      <div class="sec">Attention</div>
      <SettingRow
        key="notify.needs_you"
        name="Notify when an agent needs me"
        description="Desktop notification for approvals and questions in items you are not looking at."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <button
            type="button"
            class="switch"
            class:on={entry?.value === true}
            role="switch"
            aria-checked={entry?.value === true}
            aria-label="Notify when an agent needs me"
            disabled={pending}
            onclick={() => write(entry?.value !== true)}
          ></button>
        {/snippet}
      </SettingRow>
      <SettingRow
        key="notify.turn_finished"
        name="Notify when a turn finishes"
        description="Off by default: the unread dot is usually enough."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <button
            type="button"
            class="switch"
            class:on={entry?.value === true}
            role="switch"
            aria-checked={entry?.value === true}
            aria-label="Notify when a turn finishes"
            disabled={pending}
            onclick={() => write(entry?.value !== true)}
          ></button>
        {/snippet}
      </SettingRow>

      <div class="sec">Power</div>
      <SettingRow
        key="power.keep_awake"
        name="Keep the computer awake"
        description="Blocks idle sleep while an OMP agent works or anything runs. macOS may sleep the display; Linux also blocks the idle lock. A closed lid still sleeps."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <span class="seg">
            {#each [["off", "Off"], ["working", "While working"], ["running", "While running"]] as [value, label] (value)}
              <button
                type="button"
                class:on={(entry?.value ?? "off") === value}
                disabled={pending}
                aria-pressed={(entry?.value ?? "off") === value}
                onclick={() => write(value)}>{label}</button
              >
            {/each}
          </span>
        {/snippet}
      </SettingRow>

      <div class="sec">Terminals</div>
      <SettingRow
        key="terminal.font.size"
        fileKey="font.size"
        name="Font size"
        description="Embedded terminal only; your global Alacritty config is not read or changed."
        timing="live"
      >
        {#snippet control(entry, write, pending)}
          <NumberField
            value={typeof entry?.value === "number" ? entry.value : undefined}
            unit="pt"
            label="Terminal font size"
            min={6}
            max={72}
            step={0.5}
            disabled={pending}
            oncommit={write}
          />
        {/snippet}
      </SettingRow>
      <SettingRow
        key="terminals.remove_on_clean_exit"
        name="Remove shells that exit cleanly"
        description="Exit code 0 removes the row; any other code keeps the final screen."
        timing="next exit"
      >
        {#snippet control(entry, write, pending)}
          <button
            type="button"
            class="switch"
            class:on={entry?.value === true}
            role="switch"
            aria-checked={entry?.value === true}
            aria-label="Remove shells that exit cleanly"
            disabled={pending}
            onclick={() => write(entry?.value !== true)}
          ></button>
        {/snippet}
      </SettingRow>

      <div class="sec">In the files only</div>
      <div class="fileonly">
        {#each ["terminals.confirm_close_with_foreground_job", "confirm.close_unmonitored_agents", "resume.open_elsewhere_hint_seconds", "tray.icon"] as key (key)}
          {@const entry = settings.values.get(key)}
          <div class="fo">
            <span class="key code">{key}</span>
            <span class="v"
              >{entry === undefined
                ? "—"
                : Array.isArray(entry.value)
                  ? entry.value.join(", ")
                  : String(entry.value)}</span
            >
            <span class="dim">{entry?.source == null ? "default" : fileName(entry.source.file)}</span>
            {#if entry?.source != null}
              {@const source = entry.source}
              <button
                type="button"
                class="btn xs ghost"
                onclick={() => run("Open file", () => settingsOpenFile(source.file, source.line))}
                >Open at line {source.line}</button
              >
            {/if}
          </div>
        {/each}
        <p class="dim">
          Tools (<code>[tools.*]</code>, including <code>icon</code>, <code>icon_tint</code> and
          <code>icon_fill</code>), provider logos (<code>[providers.*] icon</code>), project roots and aliases, and
          <code>keys.toml</code> are edited as text.
        </p>
      </div>
    </div>
  </div>
  {#if workspace.active !== null}
    <div class="hidden-note">Esc returns to “{workspace.active.title}”</div>
  {/if}
</div>

<style>
  .settings {
    padding: 22px 28px 0;
    align-items: stretch;
  }
  .card {
    max-width: 760px;
    margin: 0 auto;
    gap: 10px;
    flex: 1;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 12px;
  }
  .done {
    margin-left: auto;
  }
  .dim {
    color: var(--dim);
    font-size: 12.5px;
  }
  .files {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    font-size: 12.5px;
  }
  .grow {
    flex: 1;
  }
  .key {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text);
    background: #1b1b1b;
    border: 1px solid #2f2f2f;
    border-radius: 5px;
    padding: 0 6px;
  }
  .rows {
    display: flex;
    flex-direction: column;
    flex: 1;
    padding-bottom: 36px;
  }
  .rows .banner {
    margin-bottom: 8px;
  }
  .sec {
    padding-top: 18px;
  }
  .set.ui {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 4px 16px;
    padding: 12px 4px;
    border-bottom: 1px solid var(--line);
    align-items: center;
  }
  .set.ui .n {
    color: var(--hi);
    font-size: 13.5px;
  }
  .set.ui .d {
    font-size: 12px;
    color: var(--dim);
    grid-column: 1;
  }
  .set.ui .ctl {
    grid-row: 1 / span 2;
    grid-column: 2;
  }
  .set.ui .meta {
    grid-column: 1 / -1;
    font-size: 11.5px;
    color: var(--dim);
  }
  .pins {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    justify-content: flex-end;
    max-width: 360px;
  }
  .unpin {
    display: grid;
    place-items: center;
    color: var(--dim);
  }
  .unpin:hover {
    color: var(--hi);
  }
  .fileonly {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 4px;
  }
  .fo {
    display: flex;
    gap: 10px;
    align-items: center;
    font-size: 12px;
  }
  .fo .v {
    color: var(--hi);
  }
  code {
    font-family: var(--mono);
    font-size: 11.5px;
  }
</style>
