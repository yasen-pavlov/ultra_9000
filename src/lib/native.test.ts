import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  appHide,
  appQuit,
  appRequestQuit,
  conversationsList,
  errorMessage,
  fetchSettingsState,
  fetchWorkspaceState,
  focusTerminal,
  itemClose,
  itemLaunch,
  itemRestart,
  itemResume,
  itemSelect,
  lastSetDismiss,
  lastSetResume,
  listenConversationsChanged,
  listenSettingsState,
  listenUiCommand,
  listenWindowFocus,
  listenWorkspaceState,
  noticeDismiss,
  pathCheck,
  pickDirectory,
  setTerminalViewport,
  settingsOpenFile,
  settingsReset,
  settingsReveal,
  settingsSet,
  sheetClose,
  sheetOpen,
  uiSet,
} from "./native";

afterEach(() => {
  clearMocks();
});

/** Records every command the web view sends; each resolves to `reply`. */
function recordCommands(reply: unknown = null): { cmd: string; args: unknown }[] {
  const calls: { cmd: string; args: unknown }[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    return reply;
  });
  return calls;
}

describe("command bindings", () => {
  // Argument names are the contract with the Rust #[tauri::command] parameters (camelCase on the
  // wire, snake_case in Rust). A renamed key silently fails deserialization in native.
  const rect = { x: 1, y: 2, width: 300, height: 200, topLeftRadius: 8, bottomLeftRadius: 0 };
  const cases: [string, () => Promise<unknown>, string, Record<string, unknown>][] = [
    ["fetchWorkspaceState", () => fetchWorkspaceState(), "workspace_state", {}],
    ["itemLaunch", () => itemLaunch("omp", "/w/atlas"), "item_launch", { tool: "omp", cwd: "/w/atlas" }],
    [
      "itemLaunch with anchor",
      () => itemLaunch("shell", "/w/atlas", "item-7"),
      "item_launch",
      { tool: "shell", cwd: "/w/atlas", anchorItemId: "item-7" },
    ],
    [
      "itemResume",
      () => itemResume("claude", "conv-1", "/s/conv-1.jsonl"),
      "item_resume",
      { tool: "claude", conversationId: "conv-1", path: "/s/conv-1.jsonl" },
    ],
    ["itemRestart", () => itemRestart("item-3"), "item_restart", { itemId: "item-3" }],
    ["itemSelect", () => itemSelect("item-3"), "item_select", { itemId: "item-3" }],
    ["itemClose", () => itemClose("item-3", true), "item_close", { itemId: "item-3", confirmed: true }],
    ["noticeDismiss", () => noticeDismiss("n-1"), "notice_dismiss", { noticeId: "n-1" }],
    [
      "sheetOpen",
      () => sheetOpen({ kind: "palette", projectId: null, mode: "new" }),
      "sheet_open",
      { sheet: { kind: "palette", projectId: null, mode: "new" } },
    ],
    ["sheetClose", () => sheetClose(), "sheet_close", {}],
    ["appHide", () => appHide(), "app_hide", {}],
    ["appRequestQuit", () => appRequestQuit(), "app_request_quit", {}],
    ["appQuit", () => appQuit(), "app_quit", {}],
    ["lastSetResume", () => lastSetResume(), "last_set_resume", {}],
    ["lastSetDismiss", () => lastSetDismiss(), "last_set_dismiss", {}],
    ["uiSet", () => uiSet({ foldedProjectIds: ["/w/a"] }), "ui_set", { foldedProjectIds: ["/w/a"] }],
    ["pathCheck", () => pathCheck("~/w"), "path_check", { path: "~/w" }],
    ["pickDirectory", () => pickDirectory("/w"), "pick_directory", { initialDirectory: "/w" }],
    ["conversationsList default", () => conversationsList(), "conversations_list", {}],
    [
      "conversationsList filtered",
      () => conversationsList({ projectId: "/w/a", query: "fix", limit: 5 }),
      "conversations_list",
      { projectId: "/w/a", query: "fix", limit: 5 },
    ],
    ["fetchSettingsState", () => fetchSettingsState(), "settings_state", {}],
    [
      "settingsSet",
      () => settingsSet("sidebar.width", 280, { project: "/w/a" }),
      "settings_set",
      { key: "sidebar.width", value: 280, target: { project: "/w/a" } },
    ],
    [
      "settingsReset",
      () => settingsReset("sidebar.width", "machine"),
      "settings_reset",
      { key: "sidebar.width", target: "machine" },
    ],
    ["settingsOpenFile", () => settingsOpenFile("config.toml"), "settings_open_file", { file: "config.toml" }],
    [
      "settingsOpenFile at line",
      () => settingsOpenFile("config.toml", 12),
      "settings_open_file",
      { file: "config.toml", line: 12 },
    ],
    ["settingsReveal", () => settingsReveal(), "settings_reveal", {}],
    [
      "setTerminalViewport",
      () => setTerminalViewport("item-3", 4, rect, true, 99),
      "terminal_set_viewport",
      { itemId: "item-3", presentationEpoch: 4, rect, visible: true, sequence: 99 },
    ],
    [
      "focusTerminal",
      () => focusTerminal("item-3", 4, 99),
      "terminal_focus",
      { itemId: "item-3", presentationEpoch: 4, sequence: 99 },
    ],
  ];

  it.each(cases)("%s sends its contract command and arguments", async (_name, call, cmd, args) => {
    const calls = recordCommands();
    await call();
    // toStrictEqual: an omitted optional must be absent, not present as undefined.
    expect(calls).toStrictEqual([{ cmd, args }]);
  });

  it("resolves native's reply unchanged", async () => {
    recordCommands("confirm");
    await expect(itemClose("item-3", false)).resolves.toBe("confirm");
    recordCommands(null);
    await expect(pickDirectory("/w")).resolves.toBeNull();
  });

  it("rejects with native's error so callers can surface it", async () => {
    mockIPC(() => Promise.reject(new Error("unknown tool: vim")));
    await expect(itemLaunch("vim", "/w")).rejects.toThrow("unknown tool: vim");
  });
});

describe("errorMessage", () => {
  it("passes native string errors through", () => {
    expect(errorMessage("path does not exist")).toBe("path does not exist");
  });

  it("uses an Error's message", () => {
    expect(errorMessage(new TypeError("bad input"))).toBe("bad input");
  });

  it("uses a string message field of a plain object", () => {
    expect(errorMessage({ message: "store unreadable", code: 5 })).toBe("store unreadable");
  });

  it("serializes objects without a string message", () => {
    expect(errorMessage({ message: 5 })).toBe('{"message":5}');
    expect(errorMessage(null)).toBe("null");
    expect(errorMessage(42)).toBe("42");
  });

  it("falls back to String() for values JSON cannot serialize", () => {
    expect(errorMessage(10n)).toBe("10");
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(errorMessage(cyclic)).toBe("[object Object]");
  });
});

describe("event bindings", () => {
  const listeners: [string, (handler: (payload: unknown) => void) => Promise<() => void>, string, unknown][] = [
    ["listenWorkspaceState", listenWorkspaceState, "workspace-state", { revision: 3 }],
    ["listenSettingsState", listenSettingsState, "settings-state", { revision: 2 }],
    ["listenConversationsChanged", listenConversationsChanged, "conversations-changed", { revision: 9 }],
    ["listenUiCommand", listenUiCommand, "ui-command", { command: "focus-sidebar" }],
  ];

  it.each(listeners)(
    "%s hands the %s payload to the handler until unlistened",
    async (_name, subscribe, event, payload) => {
      mockIPC(() => null, { shouldMockEvents: true });
      const handler = vi.fn();
      const unlisten = await subscribe(handler);
      await emit(event, payload);
      await emit("some-other-event", { revision: 100 });
      expect(handler.mock.calls).toEqual([[payload]]);

      // The mock keeps the stale registration and warns when it finds no callback for it.
      const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
      unlisten();
      await emit(event, payload);
      expect(handler).toHaveBeenCalledTimes(1);
      expect(warn).toHaveBeenCalled();
    },
  );

  it("listenWindowFocus reports key-window gain as true and loss as false", async () => {
    mockWindows("main");
    mockIPC(() => null, { shouldMockEvents: true });
    const handler = vi.fn();
    await listenWindowFocus(handler);
    await emit("tauri://blur");
    await emit("tauri://focus");
    expect(handler.mock.calls).toEqual([[false], [true]]);
  });
});
