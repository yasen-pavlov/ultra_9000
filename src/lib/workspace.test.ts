import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { deferred, itemState, projectState, settle, toolInfo, workspaceState } from "../test/fixtures";
import { clockTime } from "./format";
import type { WorkspaceState } from "./native";
import type * as WorkspaceModule from "./workspace.svelte";

let workspace: typeof WorkspaceModule.workspace;

beforeEach(async () => {
  // The module exports a singleton; re-importing after a registry reset gives each test a fresh
  // store, which a static import cannot.
  vi.resetModules();
  ({ workspace } = await import("./workspace.svelte"));
});

afterEach(() => {
  clearMocks();
});

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

describe("derived presentation", () => {
  const atlas = projectState({ id: "/w/atlas", itemIds: ["a1", "a2", "gone"] });
  const beacon = projectState({ id: "/w/beacon", itemIds: ["b1"] });
  const items = [
    itemState({ id: "a1", projectId: "/w/atlas", title: "Fix", startedAt: 1_000_000 }),
    itemState({ id: "a2", projectId: "/w/atlas", title: "Fix", startedAt: 9_000_000 }),
    itemState({ id: "b1", projectId: "/w/beacon", title: "Fix" }),
  ];

  it("is empty before the first snapshot", () => {
    expect(workspace.groups).toEqual([]);
    expect(workspace.active).toBeNull();
    expect(workspace.terminalVisible).toBe(false);
    expect(workspace.previousItemId).toBeNull();
    expect(workspace.waiting).toEqual([]);
  });

  it("groups items by project in native order and skips ids without an item", () => {
    workspace.state = workspaceState({ projects: [beacon, atlas], items });
    expect(workspace.groups.map((group) => [group.project.id, group.items.map((item) => item.id)])).toEqual([
      ["/w/beacon", ["b1"]],
      ["/w/atlas", ["a1", "a2"]],
    ]);
  });

  it("reaches only unfolded groups by number", () => {
    workspace.state = workspaceState({
      projects: [atlas, beacon],
      items,
      ui: { sidebarExpanded: true, foldedProjectIds: ["/w/atlas"] },
    });
    expect(workspace.groups.map((group) => group.folded)).toEqual([true, false]);
    expect(workspace.visibleOrder.map((item) => item.id)).toEqual(["b1"]);
  });

  it("adds a start-time suffix only to items sharing a title within one project", () => {
    workspace.state = workspaceState({ projects: [atlas, beacon], items });
    expect(workspace.disambiguation.get("a1")).toBe(` · ${clockTime(1_000_000)}`);
    expect(workspace.disambiguation.get("a2")).toBe(` · ${clockTime(9_000_000)}`);
    expect(workspace.disambiguation.has("b1")).toBe(false);
  });

  it("orders waiting items: needs-you oldest first, then unread your-turn oldest first", () => {
    workspace.state = workspaceState({
      items: [
        itemState({ id: "turn-new", activity: "your-turn", unread: true, stateSince: 50 }),
        itemState({ id: "need-new", activity: "needs-you", stateSince: 40 }),
        itemState({ id: "turn-seen", activity: "your-turn", unread: false, stateSince: 1 }),
        itemState({ id: "turn-old", activity: "your-turn", unread: true, stateSince: 10 }),
        itemState({ id: "need-old", activity: "needs-you", stateSince: 30 }),
        itemState({ id: "working", activity: "working", stateSince: 0 }),
      ],
    });
    expect(workspace.waiting.map((item) => item.id)).toEqual(["need-old", "need-new", "turn-old", "turn-new"]);
    expect(workspace.needsYouCount).toBe(2);
    expect(workspace.unreadCount).toBe(2);
  });

  it("resolves the active item and shows the terminal only for a live or exited surface", () => {
    const running = itemState({ id: "x", process: "running" });
    workspace.state = workspaceState({ items: [running], activeItemId: "x" });
    expect(workspace.active).toEqual(running);
    expect(workspace.terminalVisible).toBe(true);

    workspace.state = workspaceState({ revision: 2, items: [{ ...running, process: "exited" }], activeItemId: "x" });
    expect(workspace.terminalVisible).toBe(true);

    for (const process of ["starting", "failed"] as const) {
      workspace.state = workspaceState({ items: [{ ...running, process }], activeItemId: "x" });
      expect(workspace.terminalVisible).toBe(false);
    }

    workspace.state = workspaceState({
      items: [running],
      activeItemId: "x",
      view: "sheet",
      sheet: { kind: "settings" },
    });
    expect(workspace.terminalVisible).toBe(false);
  });

  it("treats an active id without an item as no active item", () => {
    workspace.state = workspaceState({ items: [], activeItemId: "ghost" });
    expect(workspace.active).toBeNull();
    expect(workspace.terminalVisible).toBe(false);
  });

  it("picks the most recent other item that still exists as the previous item", () => {
    workspace.state = workspaceState({
      items: [itemState({ id: "a" }), itemState({ id: "c" })],
      activeItemId: "a",
      recentItemIds: ["a", "closed", "c"],
    });
    expect(workspace.previousItemId).toBe("c");

    workspace.state = workspaceState({ items: [itemState({ id: "a" })], activeItemId: "a", recentItemIds: ["a"] });
    expect(workspace.previousItemId).toBeNull();
  });

  it("looks up shortcut labels, tools and labels with fallbacks", () => {
    workspace.state = workspaceState({
      shortcuts: [
        { action: "palette", label: "Ctrl+Shift+Space" },
        { action: "hide", label: null },
      ],
      tools: [toolInfo({ id: "codex", label: "Codex" })],
    });
    expect(workspace.shortcut("palette")).toBe("Ctrl+Shift+Space");
    expect(workspace.shortcut("hide")).toBeNull();
    expect(workspace.shortcut("quit")).toBeNull();
    expect(workspace.tool("codex")?.label).toBe("Codex");
    expect(workspace.tool("aider")).toBeUndefined();
    expect(workspace.toolLabel("codex")).toBe("Codex");
    expect(workspace.toolLabel("aider")).toBe("aider");
  });

  it("names projects from native state, else from the folder", () => {
    workspace.state = workspaceState({ projects: [projectState({ id: "/w/atlas", name: "Atlas" })] });
    expect(workspace.projectName("/w/atlas")).toBe("Atlas");
    expect(workspace.projectName("/w/beacon//")).toBe("beacon");
    expect(workspace.projectName("/")).toBe("/");
    expect(workspace.projectName("//")).toBe("//");
    expect(workspace.projectName("beacon")).toBe("beacon");
    expect(workspace.projectName("")).toBe("");
    expect(workspace.projectName(null)).toBe("");
  });

  it("names folders in linear time on long slash runs", () => {
    workspace.state = workspaceState();
    const started = performance.now();
    expect(workspace.projectName(`${"/".repeat(200_000)}x`)).toBe("x");
    expect(workspace.projectName(`/w/beacon${"/".repeat(200_000)}`)).toBe("beacon");
    expect(performance.now() - started).toBeLessThan(250);
  });
});

describe("connect", () => {
  /** Native with mocked events; `workspace_state` answers with `reply()`. */
  function mockNative(reply: () => unknown): string[] {
    const commands: string[] = [];
    mockIPC(
      (cmd) => {
        commands.push(cmd);
        if (cmd === "workspace_state") return reply();
        throw new Error(`unexpected command ${cmd}`);
      },
      { shouldMockEvents: true },
    );
    return commands;
  }

  it("applies the initial snapshot and remembers the active item's project", async () => {
    const commands = mockNative(() =>
      workspaceState({ revision: 4, items: [itemState({ id: "x", projectId: "/w/atlas" })], activeItemId: "x" }),
    );
    workspace.connect();
    await settle();
    expect(commands).toEqual(["workspace_state"]);
    expect(workspace.state?.revision).toBe(4);
    expect(workspace.lastProjectId).toBe("/w/atlas");
    expect(workspace.error).toBeNull();
  });

  it("subscribes before fetching, so a push during the fetch is kept over the older reply", async () => {
    mockNative(() => {
      // Native pushes a newer state while the read is in flight.
      void emit("workspace-state", workspaceState({ revision: 7 }));
      return workspaceState({ revision: 6 });
    });
    workspace.connect();
    await settle();
    expect(workspace.state?.revision).toBe(7);
  });

  it("applies only strictly newer revisions and keeps the last project when nothing is active", async () => {
    mockNative(() =>
      workspaceState({ revision: 2, items: [itemState({ id: "x", projectId: "/w/atlas" })], activeItemId: "x" }),
    );
    workspace.connect();
    await settle();

    await emit("workspace-state", workspaceState({ revision: 2, activeItemId: null, defaultCwd: "/same-revision" }));
    expect(workspace.state?.defaultCwd).toBe("/home/ana");

    await emit("workspace-state", workspaceState({ revision: 3, activeItemId: null }));
    expect(workspace.state?.revision).toBe(3);
    expect(workspace.lastProjectId).toBe("/w/atlas");

    await emit("workspace-state", workspaceState({ revision: 1 }));
    expect(workspace.state?.revision).toBe(3);
  });

  it("reports a failed read when no state arrived", async () => {
    mockNative(() => Promise.reject(new Error("main thread gone")));
    workspace.connect();
    await settle();
    expect(workspace.error).toBe("Cannot read workspace state: main thread gone");
  });

  it("ignores a failed read once a pushed state was applied", async () => {
    mockNative(() => {
      void emit("workspace-state", workspaceState({ revision: 3 }));
      return Promise.reject(new Error("late failure"));
    });
    workspace.connect();
    await settle();
    expect(workspace.state?.revision).toBe(3);
    expect(workspace.error).toBeNull();
  });

  it("reports a failed subscription and does not read without one", async () => {
    const commands: string[] = [];
    mockIPC((cmd) => {
      commands.push(cmd);
      if (cmd === "plugin:event|listen") return Promise.reject(new Error("event plugin missing"));
      return workspaceState();
    });
    workspace.connect();
    await settle();
    expect(workspace.error).toBe("Cannot subscribe to workspace state: event plugin missing");
    expect(commands).toEqual(["plugin:event|listen"]);
  });

  it("clears a newer snapshot's error and stops applying pushes after cleanup", async () => {
    mockNative(() => Promise.reject(new Error("busy")));
    const cleanup = workspace.connect();
    const events = recordEventCommands();
    await settle();
    expect(workspace.error).not.toBeNull();

    await emit("workspace-state", workspaceState({ revision: 5 }));
    expect(workspace.error).toBeNull();

    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    cleanup();
    expect(events).toEqual(["plugin:event|unlisten"]);
    await emit("workspace-state", workspaceState({ revision: 6 }));
    expect(workspace.state?.revision).toBe(5);
    warn.mockRestore();
  });

  it("releases a subscription that completes after cleanup and skips the read", async () => {
    const listen = deferred<number>();
    const commands: string[] = [];
    mockIPC((cmd) => {
      commands.push(cmd);
      return cmd === "plugin:event|listen" ? listen.promise : null;
    });
    const cleanup = workspace.connect();
    cleanup();
    listen.resolve(17);
    await settle();
    expect(commands).toEqual(["plugin:event|listen", "plugin:event|unlisten"]);
  });

  it("drops a read that completes after cleanup", async () => {
    const read = deferred<WorkspaceState>();
    mockNative(() => read.promise);
    const cleanup = workspace.connect();
    await settle();
    cleanup();
    read.resolve(workspaceState({ revision: 9 }));
    await settle();
    expect(workspace.state).toBeNull();
  });

  it("retry clears the error and reconnects, replacing the previous connection", async () => {
    let fail = true;
    const commands = mockNative(() => (fail ? Promise.reject(new Error("busy")) : workspaceState({ revision: 2 })));
    const cleanup = workspace.connect();
    const events = recordEventCommands();
    await settle();
    expect(workspace.error).toBe("Cannot read workspace state: busy");

    fail = false;
    workspace.retry();
    expect(workspace.error).toBeNull();
    await settle();
    expect(workspace.state?.revision).toBe(2);
    expect(commands).toEqual(["workspace_state", "workspace_state"]);
    expect(events).toEqual(["plugin:event|unlisten"]);

    // The original cleanup releases whichever connection is current (the retried one).
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    cleanup();
    expect(events).toEqual(["plugin:event|unlisten", "plugin:event|unlisten"]);
    await emit("workspace-state", workspaceState({ revision: 3 }));
    expect(workspace.state?.revision).toBe(2);
    warn.mockRestore();
  });
});
