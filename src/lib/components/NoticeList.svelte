<script lang="ts">
  // Native notices (an agent ended: Resume; a launch/action failed) plus local command failures,
  // shown in the expanded sidebar above the utilities. Never a toast over the terminal.
  import { run, uiErrors } from "../errors.svelte";
  import { itemResume, noticeDismiss, type Notice } from "../native";
  import { workspace } from "../workspace.svelte";

  let { viewportError }: { viewportError: string | null } = $props();

  const notices = $derived(workspace.state?.notices ?? []);

  function resume(notice: Notice): void {
    const conversation = notice.conversation;
    if (conversation === null) return;
    void run("Resume", () => itemResume(conversation.tool, conversation.id, conversation.path));
  }
</script>

{#if notices.length > 0 || uiErrors.list.length > 0 || viewportError !== null}
  <div class="notices scroll">
    {#each notices as notice (notice.id)}
      <div class="notice" class:error={notice.kind === "error"} role="status">
        <span class="h">{notice.title}</span>
        {#if notice.body !== ""}<span class="b">{notice.body}</span>{/if}
        <span class="acts">
          {#if notice.conversation !== null}
            <button type="button" class="btn sm" onclick={() => resume(notice)}>Resume</button>
          {/if}
          <button type="button" class="btn sm ghost" onclick={() => run("Dismiss", () => noticeDismiss(notice.id))}
            >Dismiss</button
          >
        </span>
      </div>
    {/each}
    {#each uiErrors.list as entry (entry.id)}
      <div class="notice error" role="alert">
        <span class="b selectable">{entry.message}</span>
        <span class="acts"
          ><button type="button" class="btn sm ghost" onclick={() => uiErrors.dismiss(entry.id)}>Dismiss</button></span
        >
      </div>
    {/each}
    {#if viewportError !== null}
      <div class="notice error" role="alert"><span class="b selectable">{viewportError}</span></div>
    {/if}
  </div>
{/if}

<style>
  .notices {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: none;
    max-height: 40%;
  }
  .notice {
    flex: none;
    border-radius: 8px;
    background: #222;
    border: 1px solid var(--line);
    padding: 8px 10px;
    font-size: 12px;
    color: var(--text);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .notice.error {
    border-color: #5a2430;
    background: #2a171b;
  }
  .h {
    color: var(--hi);
  }
  .b {
    overflow-wrap: anywhere;
  }
  .acts {
    display: flex;
    gap: 6px;
  }
</style>
