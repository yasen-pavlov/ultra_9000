<script lang="ts">
  // Wide-palette detail pane: only what the session file records (or what the running item
  // reports). Messages, cost and context are never read. For a recent document: its H1, metadata
  // and first paragraph, from the first 64 KB only (board 33).
  import { clock } from "../clock.svelte";
  import { formatBytes, fullDate, modelShort, spokenAge, tildePath } from "../format";
  import { itemView } from "../items";
  import {
    documentPreview,
    errorMessage,
    type Conversation,
    type DocumentEntry,
    type DocumentPreview,
    type ItemState,
  } from "../native";
  import { workspace } from "../workspace.svelte";
  import ProviderMark from "./ProviderMark.svelte";

  let {
    conversation,
    item,
    document = null,
    onresume,
    onswitch,
    onview,
    onneovim,
  }: {
    conversation: Conversation | null;
    item: ItemState | null;
    document?: DocumentEntry | null;
    onresume: (conversation: Conversation) => void;
    onswitch: (item: ItemState) => void;
    onview?: () => void;
    onneovim?: () => void;
  } = $props();

  const home = $derived(workspace.state?.defaultCwd ?? "");

  let preview = $state<{ path: string; value: DocumentPreview | null; error: string | null } | null>(null);

  // The preview follows the cursor; a late answer for a previous row is dropped.
  $effect(() => {
    const path = document?.path ?? null;
    if (path === null) return;
    preview = { path, value: null, error: null };
    documentPreview(path).then(
      (value) => {
        if (preview?.path === path) preview = { path, value, error: null };
      },
      (error) => {
        if (preview?.path === path) preview = { path, value: null, error: errorMessage(error) };
      },
    );
  });
  const shown = $derived(document !== null && preview?.path === document.path ? preview : null);
</script>

<div class="det" aria-label="Details">
  {#if document !== null}
    {@const headings = shown?.value?.headingCount ?? null}
    <h3>{shown?.value?.h1 ?? document.name}</h3>
    <div class="kv">
      <span class="k">File</span><span class="v">{document.name}</span>
      <span class="k">Folder</span><span class="v m selectable" title={document.path}>{document.dir}</span>
      <span class="k">Modified</span><span class="v">{spokenAge(document.modifiedMs, clock.now)} ago</span>
      <span class="k">Size</span><span class="v"
        >{formatBytes(document.sizeBytes)}{headings === null
          ? ""
          : ` · ${headings} ${headings === 1 ? "heading" : "headings"}`}</span
      >
    </div>
    {#if shown?.error != null}
      <p class="warn">{shown.error}</p>
    {:else if shown?.value?.firstParagraph != null}
      <p class="para selectable">{shown.value.firstParagraph}</p>
    {/if}
    <div class="acts">
      <button type="button" class="btn pri" onclick={onview}>View<kbd>↵</kbd></button>
      <button type="button" class="btn" onclick={onneovim}>Neovim<kbd>Ctrl+↵</kbd></button>
    </div>
    <p class="note">Shown: the file name, its metadata and the first 64 KB. Who wrote it is not known.</p>
  {:else if conversation !== null}
    <h3>{conversation.title}</h3>
    <div class="kv">
      <span class="k">Project</span><span class="v">{workspace.projectName(conversation.projectId)}</span>
      <span class="k">Folder</span><span class="v m selectable" title={conversation.cwd}
        >{tildePath(conversation.cwd, home)}</span
      >
      <span class="k">Tool</span><span class="v">{workspace.toolLabel(conversation.tool)}</span>
      <span class="k">Started</span><span class="v">{fullDate(conversation.createdAt)}</span>
      <span class="k">Last write</span><span class="v">{fullDate(conversation.updatedAt)}</span>
      {#if conversation.model !== null}
        <span class="k">Model</span><span class="v"
          ><ProviderMark model={conversation.model} tool={conversation.tool} inline />{modelShort(
            conversation.model,
          )}</span
        >
      {/if}
      <span class="k">Id</span><span class="v m selectable" title={conversation.id}>{conversation.id}</span>
      <span class="k">File</span><span class="v m selectable" title={conversation.path}
        >{tildePath(conversation.path, home)}</span
      >
    </div>
    {#if conversation.cwdMissing}
      <p class="warn">
        The recorded folder is gone. The tool resumes only in its recorded folder, and Ultra 9000 won't guess another
        one.
      </p>
    {:else if conversation.maybeOpenElsewhere}
      <p class="warn">
        Written {spokenAge(conversation.updatedAt, clock.now)} ago but not running here: it may be open in another terminal.
      </p>
    {/if}
    <div class="acts">
      <button type="button" class="btn pri" disabled={conversation.cwdMissing} onclick={() => onresume(conversation)}>
        Resume<kbd>↵</kbd>
      </button>
    </div>
    <p class="note">
      Shown: what the session file records. Last write is the file's modification time, an approximation of last
      activity. Not shown: messages, cost, context — not read.
    </p>
  {:else if item !== null}
    {@const view = itemView(item, { tool: workspace.tool(item.tool), now: clock.now, home })}
    <h3>{view.title}</h3>
    <div class="kv">
      <span class="k">Project</span><span class="v">{workspace.projectName(item.projectId)}</span>
      <span class="k">Folder</span><span class="v m selectable" title={item.cwd}>{tildePath(item.cwd, home)}</span>
      <span class="k">Tool</span><span class="v">{workspace.toolLabel(item.tool)}</span>
      <span class="k">State</span><span class="v">{view.stateText}</span>
      <span class="k">Started</span><span class="v">{fullDate(item.startedAt)}</span>
      {#if item.model !== null}
        <span class="k">Model</span><span class="v"
          ><ProviderMark model={item.model} tool={item.tool} inline />{modelShort(item.model)}</span
        >
      {/if}
      {#if item.conversation !== null}
        <span class="k">Id</span><span class="v m selectable" title={item.conversation.id}>{item.conversation.id}</span>
      {/if}
    </div>
    <div class="acts">
      <button type="button" class="btn pri" onclick={() => onswitch(item)}>Switch<kbd>↵</kbd></button>
      {#if onneovim !== undefined}
        <button type="button" class="btn" onclick={onneovim}>Neovim<kbd>Ctrl+↵</kbd></button>
      {/if}
    </div>
    <p class="note">
      Running here: switching shows its {item.kind === "document" ? "document" : "terminal"}. It is never {item.kind ===
      "document"
        ? "opened"
        : "resumed"} twice.
    </p>
  {/if}
</div>

<style>
  .det {
    background: #1c1c1c;
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    font-size: 12.5px;
    min-width: 0;
    align-self: start;
  }
  h3 {
    margin: 0;
    font-size: 15px;
    color: var(--hi);
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .kv {
    display: grid;
    grid-template-columns: 80px minmax(0, 1fr);
    gap: 6px 10px;
  }
  .k {
    color: var(--dim);
  }
  .v {
    color: var(--hi);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .v.m {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text);
  }
  .acts {
    display: flex;
    gap: 8px;
  }
  .warn {
    margin: 0;
    color: var(--yellow);
    line-height: 1.45;
  }
  .note {
    margin: 0;
    color: var(--dim);
    font-size: 11.5px;
    line-height: 1.45;
  }
  .para {
    margin: 0;
    color: var(--text);
    line-height: 1.5;
    display: -webkit-box;
    -webkit-line-clamp: 6;
    line-clamp: 6;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
</style>
