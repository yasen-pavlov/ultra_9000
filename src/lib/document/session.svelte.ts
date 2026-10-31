// Per-document reader memory for this app run (board 34): place, follow and find text survive
// switching away and back; the parsed H1 feeds the context bar. Nothing here is persisted or sent
// to native. The outline rail's hidden state is one app-wide preference.
import type { Place } from "./place";

export interface DocumentMemory {
  place: Place | null;
  /** null until the first render applied ctl's `--follow` request. */
  follow: boolean | null;
  findText: string;
  linesBelow: number;
  /** `document_asset` replies as data: URLs by relative path; cleared by reload. */
  assets: Map<string, Promise<string | null>>;
  /** The last text shown and its revision: shown again at once after switching back, also under a missing/unreadable notice. */
  text: string | null;
  revision: number;
}

class DocumentSessions {
  /** Parsed first H1 by item id (the context bar's “dir · H1”). */
  h1 = $state<Record<string, string | null>>({});
  /** `o` pressed on a showing rail hides it; remembered for every document. */
  railHidden = $state(false);
  #memory = new Map<string, DocumentMemory>();

  memory(itemId: string): DocumentMemory {
    let memory = this.#memory.get(itemId);
    if (memory === undefined) {
      memory = { place: null, follow: null, findText: "", linesBelow: 0, assets: new Map(), text: null, revision: -1 };
      this.#memory.set(itemId, memory);
    }
    return memory;
  }

  setH1(itemId: string, h1: string | null): void {
    if (this.h1[itemId] !== h1) this.h1[itemId] = h1;
  }

  /** Drops memory of documents that are no longer open. */
  retain(openIds: ReadonlySet<string>): void {
    for (const id of this.#memory.keys()) if (!openIds.has(id)) this.#memory.delete(id);
    for (const id of Object.keys(this.h1)) if (!openIds.has(id)) delete this.h1[id];
  }
}

export const documentSessions = new DocumentSessions();
