import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { conversationList, deferred, settle } from "../test/fixtures";
import type { Conversation, ConversationList } from "./native";
import type * as ConversationsModule from "./conversations.svelte";

let conversations: typeof ConversationsModule.conversations;

beforeEach(async () => {
  // The module exports a singleton; re-importing after a registry reset gives each test a fresh
  // store, which a static import cannot.
  vi.resetModules();
  ({ conversations } = await import("./conversations.svelte"));
});

afterEach(() => {
  clearMocks();
});

function conversation(id: string, projectId: string, updatedAt: number): Conversation {
  return {
    tool: "omp",
    id,
    path: `/s/${id}.jsonl`,
    title: id,
    projectId,
    cwd: projectId,
    cwdMissing: false,
    createdAt: 0,
    updatedAt,
    model: null,
    runningItemId: null,
    maybeOpenElsewhere: false,
  };
}

/** Native with mocked events; each `conversations_list` call takes the next reply. */
function mockNative(...replies: (() => unknown)[]): { cmd: string; args: unknown }[] {
  const calls: { cmd: string; args: unknown }[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args });
      if (cmd !== "conversations_list") throw new Error(`unexpected command ${cmd}`);
      const reply = replies.shift();
      return reply === undefined ? conversationList() : reply();
    },
    { shouldMockEvents: true },
  );
  return calls;
}

/** Records unlisten commands, which mocked events handle without reaching the IPC callback. */
function recordEventCommands(): string[] {
  // mockIPC installs these internals; the API package does not type them on window.
  const tauriWindow = window as unknown as {
    __TAURI_INTERNALS__: { invoke: (cmd: string, ...rest: unknown[]) => unknown };
  };
  const internals = tauriWindow.__TAURI_INTERNALS__;
  const invoke = internals.invoke;
  const commands: string[] = [];
  internals.invoke = (cmd, ...rest) => {
    if (cmd === "plugin:event|unlisten") commands.push(cmd);
    return invoke(cmd, ...rest);
  };
  return commands;
}

describe("projects", () => {
  it("counts conversations per project, most recently active project first", () => {
    conversations.list = conversationList({
      conversations: [
        conversation("a", "/w/atlas", 10),
        conversation("b", "/w/beacon", 30),
        conversation("c", "/w/atlas", 50),
        conversation("d", "/w/atlas", 20),
      ],
    });
    expect(conversations.projects).toEqual([
      { projectId: "/w/atlas", count: 3, latest: 50 },
      { projectId: "/w/beacon", count: 1, latest: 30 },
    ]);
  });

  it("is empty before the first list", () => {
    expect(conversations.projects).toEqual([]);
  });
});

describe("watch", () => {
  it("lists every conversation once for the first watcher only", async () => {
    const calls = mockNative(() => conversationList({ revision: 2 }));
    conversations.watch();
    conversations.watch();
    await settle();
    expect(calls).toEqual([{ cmd: "conversations_list", args: { limit: Number.MAX_SAFE_INTEGER } }]);
    expect(conversations.list?.revision).toBe(2);
  });

  it("re-queries and bumps the change counter on conversations-changed while watched", async () => {
    const calls = mockNative(
      () => conversationList({ revision: 1 }),
      () => conversationList({ revision: 2, conversations: [conversation("a", "/w/atlas", 5)] }),
    );
    const release = conversations.watch();
    const events = recordEventCommands();
    await settle();
    await emit("conversations-changed", { revision: 2 });
    await settle();
    expect(conversations.changes).toBe(1);
    expect(conversations.list?.conversations.map((entry) => entry.id)).toEqual(["a"]);
    expect(calls).toHaveLength(2);

    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    release();
    expect(events).toEqual(["plugin:event|unlisten"]);
    await emit("conversations-changed", { revision: 3 });
    await settle();
    expect(conversations.changes).toBe(1);
    expect(calls).toHaveLength(2);
    warn.mockRestore();
  });

  it("keeps watching until the last watcher releases, and a release counts once", async () => {
    mockNative();
    const events = recordEventCommands();
    const first = conversations.watch();
    const second = conversations.watch();
    await settle();
    first();
    first();
    expect(events).toEqual([]);
    await emit("conversations-changed", { revision: 2 });
    await settle();
    expect(conversations.changes).toBe(1);

    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    second();
    expect(events).toEqual(["plugin:event|unlisten"]);
    await emit("conversations-changed", { revision: 3 });
    await settle();
    expect(conversations.changes).toBe(1);
    warn.mockRestore();
  });

  it("unsubscribes a subscription that completes after the last release", async () => {
    const listen = deferred<number>();
    const commands: string[] = [];
    mockIPC((cmd) => {
      commands.push(cmd);
      return cmd === "plugin:event|listen" ? listen.promise : conversationList();
    });
    const release = conversations.watch();
    release();
    listen.resolve(11);
    await settle();
    expect(commands).toEqual(["plugin:event|listen", "conversations_list", "plugin:event|unlisten"]);
  });

  it("reports a failed subscription, unless it was already released", async () => {
    const listen = deferred<number>();
    mockIPC((cmd) => (cmd === "plugin:event|listen" ? listen.promise : conversationList()));
    const release = conversations.watch();
    await settle();
    listen.reject(new Error("no event plugin"));
    await settle();
    expect(conversations.error).toBe("Cannot watch saved conversations: no event plugin");
    release();

    const late = deferred<number>();
    mockIPC((cmd) => (cmd === "plugin:event|listen" ? late.promise : conversationList()));
    conversations.error = null;
    conversations.watch()();
    late.reject(new Error("too late"));
    await settle();
    expect(conversations.error).toBeNull();
  });
});

describe("refresh", () => {
  it("replaces the list with equal or newer revisions only", async () => {
    mockNative(
      () => conversationList({ revision: 5, scanning: true }),
      () => conversationList({ revision: 5, scanning: false }),
      () => conversationList({ revision: 4, scanning: true }),
    );
    await conversations.refresh();
    expect(conversations.list).toMatchObject({ revision: 5, scanning: true });
    await conversations.refresh();
    expect(conversations.list).toMatchObject({ revision: 5, scanning: false });
    await conversations.refresh();
    expect(conversations.list).toMatchObject({ revision: 5, scanning: false });
  });

  it("reports a failure and clears it on the next success (Retry)", async () => {
    mockNative(
      () => Promise.reject(new Error("permission denied")),
      () => conversationList({ revision: 1 }),
    );
    await conversations.refresh();
    expect(conversations.error).toBe("Cannot list saved conversations: permission denied");
    expect(conversations.list).toBeNull();
    await conversations.refresh();
    expect(conversations.error).toBeNull();
    expect(conversations.list?.revision).toBe(1);
  });

  it("drops the result or failure of a refresh superseded by a newer one", async () => {
    const slowSuccess = deferred<ConversationList>();
    const slowFailure = deferred<ConversationList>();
    mockNative(
      () => slowSuccess.promise,
      () => slowFailure.promise,
      () => conversationList({ revision: 2 }),
    );
    const first = conversations.refresh();
    const second = conversations.refresh();
    const third = conversations.refresh();
    await third;
    slowSuccess.resolve(conversationList({ revision: 9 }));
    slowFailure.reject(new Error("stale failure"));
    await Promise.all([first, second]);
    expect(conversations.list?.revision).toBe(2);
    expect(conversations.error).toBeNull();
  });
});
