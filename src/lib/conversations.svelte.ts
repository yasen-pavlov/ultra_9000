// Saved conversations discovered read-only by native from the tools' stores. Fetched only while a
// palette or the launcher is showing; `conversations-changed` triggers a re-query while watched.
import { conversationsList, errorMessage, listenConversationsChanged, type ConversationList } from "./native";

export interface SavedProject {
  projectId: string;
  count: number;
  latest: number;
}

class ConversationsStore {
  /** Unscoped list (every project, no query), updatedAt descending. */
  list = $state.raw<ConversationList | null>(null);
  error = $state<string | null>(null);
  /** Bumps on every `conversations-changed` so scoped queries re-run. */
  changes = $state(0);

  /** Projects that have saved conversations, most recently active first. */
  projects: SavedProject[] = $derived.by(() => {
    const byProject = new Map<string, SavedProject>();
    for (const conversation of this.list?.conversations ?? []) {
      const entry = byProject.get(conversation.projectId);
      if (entry === undefined) {
        byProject.set(conversation.projectId, {
          projectId: conversation.projectId,
          count: 1,
          latest: conversation.updatedAt,
        });
      } else {
        entry.count += 1;
        entry.latest = Math.max(entry.latest, conversation.updatedAt);
      }
    }
    return [...byProject.values()].sort((a, b) => b.latest - a.latest);
  });

  #watchers = 0;
  #unlisten: (() => void) | null = null;
  /** Identifies the current subscription; a late listen() of an older one unsubscribes itself. */
  #subscription: symbol | null = null;
  #generation = 0;

  /** Keeps the list fresh while a consumer is mounted. Returns the release function. */
  watch(): () => void {
    this.#watchers += 1;
    if (this.#watchers === 1) this.#start();
    let released = false;
    return () => {
      if (released) return;
      released = true;
      this.#watchers -= 1;
      if (this.#watchers === 0) this.#stop();
    };
  }

  /** Re-reads the unscoped list (also the Retry action for unreadable stores). */
  async refresh(): Promise<void> {
    const generation = ++this.#generation;
    try {
      // Fetch all metadata for complete project choices/counts; rendered rows remain capped.
      const next = await conversationsList({ limit: Number.MAX_SAFE_INTEGER });
      if (generation !== this.#generation) return;
      if (this.list === null || next.revision >= this.list.revision) this.list = next;
      this.error = null;
    } catch (error) {
      if (generation === this.#generation) this.error = `Cannot list saved conversations: ${errorMessage(error)}`;
    }
  }

  #start(): void {
    const subscription = Symbol("conversations-changed");
    this.#subscription = subscription;
    void listenConversationsChanged(() => {
      if (this.#subscription !== subscription) return;
      this.changes += 1;
      void this.refresh();
    }).then(
      (stop) => {
        if (this.#subscription === subscription) this.#unlisten = stop;
        else stop();
      },
      (error) => {
        if (this.#subscription === subscription)
          this.error = `Cannot watch saved conversations: ${errorMessage(error)}`;
      },
    );
    void this.refresh();
  }

  #stop(): void {
    this.#subscription = null;
    this.#unlisten?.();
    this.#unlisten = null;
  }
}

export const conversations = new ConversationsStore();
