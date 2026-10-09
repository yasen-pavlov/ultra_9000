<script lang="ts">
  // One settings row (board 17): name, control, description, then the file · key that defines the
  // effective value, when it applies, its scope (this machine / all machines) and Reset. The panel
  // never holds state of its own: every change is a native write to the file, then a reload.
  import type { Snippet } from "svelte";
  import { basename } from "../format";
  import {
    errorMessage,
    settingsReset,
    settingsSet,
    type SettingScalar,
    type SettingValue,
    type WriteTarget,
  } from "../native";
  import { settings } from "../settings.svelte";

  let {
    key,
    name,
    description,
    timing,
    project = null,
    fileKey,
    control,
  }: {
    key: string;
    name: string;
    description: string;
    timing: "live" | "next launch" | "next exit";
    /** Project path for `projects."<path>".*` keys (written to that project's table). */
    project?: string | null;
    /** Key as written in its file when it differs (terminal.font.size → font.size). */
    fileKey?: string;
    control: Snippet<[SettingValue | undefined, (value: SettingScalar) => void, boolean]>;
  } = $props();

  let pending = $state(false);
  let error = $state<string | null>(null);
  let scopeOpen = $state(false);
  let scopeAnchor = $state<HTMLElement | null>(null);

  const entry = $derived(settings.values.get(key));
  const machineFile = $derived(settings.state?.files.find((file) => file.name.startsWith("machines/")) ?? null);
  const onMachine = $derived(entry?.source != null && machineFile !== null && entry.source.file === machineFile.path);
  const sourceName = $derived.by(() => {
    const source = entry?.source;
    if (source == null) return null;
    return settings.state?.files.find((file) => file.path === source.file)?.name ?? basename(source.file);
  });
  const defaultFile = $derived(key.startsWith("terminal.") ? "terminal.toml" : "ultra.toml");
  const target: WriteTarget = $derived(project === null ? "auto" : { project });

  async function apply(action: () => Promise<void>): Promise<void> {
    pending = true;
    error = null;
    try {
      await action();
    } catch (failure) {
      error = errorMessage(failure);
    } finally {
      pending = false;
    }
  }

  function write(value: SettingScalar): void {
    void apply(() => settingsSet(key, value, target));
  }

  function reset(): void {
    void apply(() => settingsReset(key, target));
  }

  /** Moves the effective value between the machine file and the shared files. */
  function setScope(machine: boolean): void {
    scopeOpen = false;
    const value = entry?.value;
    if (value === undefined || machine === onMachine) return;
    void apply(async () => {
      if (machine) {
        await settingsSet(key, value, "machine");
      } else {
        await settingsReset(key, "machine");
        await settingsSet(key, value, "auto");
      }
    });
  }

  function onwindowpointerdown(event: PointerEvent): void {
    if (scopeOpen && scopeAnchor !== null && !scopeAnchor.contains(event.target as Node)) scopeOpen = false;
  }
</script>

<svelte:window onpointerdown={onwindowpointerdown} />

<div class="set">
  <span class="n">{name}</span>
  <span class="ctl">{@render control(entry, write, pending)}</span>
  <span class="d">{description}</span>
  <span class="meta">
    <span class="key" title={entry?.source != null ? `${entry.source.file}:${entry.source.line}` : "Built-in default"}
      >{sourceName ?? defaultFile} · {fileKey ?? key}</span
    >
    {#if entry?.source == null}<span>default</span>{/if}
    <span>{timing}</span>
    {#if project === null && machineFile !== null}
      <span>·</span>
      <span class="anchor" bind:this={scopeAnchor}>
        <button
          type="button"
          class="pick scope"
          aria-haspopup="listbox"
          aria-expanded={scopeOpen}
          disabled={entry === undefined || pending}
          onclick={() => (scopeOpen = !scopeOpen)}
          onkeydown={(event) => {
            if (event.key === "Escape" && scopeOpen) {
              event.preventDefault();
              event.stopPropagation();
              scopeOpen = false;
            }
          }}
        >
          {onMachine ? "This machine" : "All machines"}
          <svg
            class="i s"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"><path d="M4 6l4 4 4-4" /></svg
          >
        </button>
        {#if scopeOpen}
          <div class="menu" role="listbox" aria-label="Scope">
            <button
              type="button"
              class="mi"
              class:sel={onMachine}
              role="option"
              aria-selected={onMachine}
              onclick={() => setScope(true)}
            >
              This machine<span class="s">{machineFile.name}</span>
            </button>
            <button
              type="button"
              class="mi"
              class:sel={!onMachine}
              role="option"
              aria-selected={!onMachine}
              onclick={() => setScope(false)}
            >
              All machines<span class="s">{defaultFile}</span>
            </button>
          </div>
        {/if}
      </span>
    {/if}
    {#if entry?.source != null}
      <button type="button" class="btn xs ghost" disabled={pending} onclick={reset}>Reset to default</button>
    {/if}
    {#if pending}<span>saving…</span>{/if}
  </span>
  {#if error !== null}
    <span class="werr selectable" role="alert">Not saved: {error}</span>
  {/if}
</div>

<style>
  .set {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 4px 16px;
    padding: 12px 4px;
    border-bottom: 1px solid var(--line);
    align-items: center;
  }
  .n {
    color: var(--hi);
    font-size: 13.5px;
  }
  .d {
    font-size: 12px;
    color: var(--dim);
    grid-column: 1;
    line-height: 1.45;
  }
  .meta {
    grid-column: 1 / -1;
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    font-size: 11.5px;
    color: var(--dim);
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
  .ctl {
    grid-row: 1 / span 2;
    grid-column: 2;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .anchor {
    position: relative;
  }
  .anchor .menu {
    top: calc(100% + 4px);
    left: 0;
    width: 260px;
  }
  .scope {
    height: 20px;
    font-size: 11.5px;
  }
  .werr {
    grid-column: 1 / -1;
    color: var(--err-text);
    font-size: 12px;
  }
</style>
