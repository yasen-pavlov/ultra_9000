import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { deferred, settingsState, settle } from "../test/fixtures";
import type { SettingScalar, SettingsState } from "./native";
import { projectToolKey } from "./settings.svelte";
import type * as SettingsModule from "./settings.svelte";

let settings: typeof SettingsModule.settings;

beforeEach(async () => {
  // The module exports a singleton; re-importing after a registry reset gives each test a fresh
  // store, which a static import cannot.
  vi.resetModules();
  ({ settings } = await import("./settings.svelte"));
});

afterEach(() => {
  clearMocks();
});

function withValues(values: Record<string, SettingScalar>, revision = 1): SettingsState {
  return settingsState({
    revision,
    values: Object.entries(values).map(([key, value]) => ({ key, value, source: null, defaultValue: value })),
  });
}

describe("projectToolKey", () => {
  it("quotes the project path as a TOML key, escaping quotes", () => {
    expect(projectToolKey("/w/atlas")).toBe('projects."/w/atlas".default_tool');
    expect(projectToolKey('/w/my "app"')).toBe('projects."/w/my \\"app\\"".default_tool');
  });
});

describe("effective values", () => {
  it("uses the built-in defaults until native state arrives", () => {
    expect(settings).toMatchObject({
      sidebarWidth: 304,
      density: "comfortable",
      providerMark: "corner",
      brandColour: "all",
      autoCollapseBelow: 800,
      contextBar: "auto",
      hasErrors: false,
    });
  });

  it("reads configured values", () => {
    settings.state = withValues({
      "sidebar.width": 280,
      "sidebar.density": "compact",
      "sidebar.provider_mark": "off",
      "icons.brand_color": "pickers",
      "sidebar.auto_collapse_below": 640,
      "layout.context_bar": "never",
    });
    expect(settings).toMatchObject({
      sidebarWidth: 280,
      density: "compact",
      providerMark: "off",
      brandColour: "pickers",
      autoCollapseBelow: 640,
      contextBar: "never",
    });
    expect(settings.values.get("sidebar.width")?.value).toBe(280);
  });

  it("clamps the sidebar width to 240–360", () => {
    settings.state = withValues({ "sidebar.width": 100 });
    expect(settings.sidebarWidth).toBe(240);
    settings.state = withValues({ "sidebar.width": 9000 }, 2);
    expect(settings.sidebarWidth).toBe(360);
  });

  it("falls back to defaults for values of the wrong type or outside the allowed set", () => {
    settings.state = withValues({
      "sidebar.width": "wide",
      "sidebar.density": "cozy",
      "sidebar.auto_collapse_below": Number.NaN,
      "layout.context_bar": true,
      "icons.brand_color": ["all"],
    });
    expect(settings).toMatchObject({
      sidebarWidth: 304,
      density: "comfortable",
      autoCollapseBelow: 800,
      contextBar: "auto",
      brandColour: "all",
    });
  });

  it("reports whether any settings file has errors", () => {
    settings.state = settingsState({
      errors: [{ file: "config.toml", line: 3, key: "sidebar.width", message: "expected a number", lastGood: "304" }],
    });
    expect(settings.hasErrors).toBe(true);
  });
});

describe("connect", () => {
  function mockNative(reply: () => unknown): string[] {
    const commands: string[] = [];
    mockIPC(
      (cmd) => {
        commands.push(cmd);
        if (cmd === "settings_state") return reply();
        throw new Error(`unexpected command ${cmd}`);
      },
      { shouldMockEvents: true },
    );
    return commands;
  }

  it("subscribes, reads the state and keeps a newer push over an older read", async () => {
    mockNative(() => {
      void emit("settings-state", withValues({ "sidebar.width": 250 }, 4));
      return withValues({ "sidebar.width": 300 }, 3);
    });
    settings.connect();
    await settle();
    expect(settings.state?.revision).toBe(4);
    expect(settings.sidebarWidth).toBe(250);

    await emit("settings-state", withValues({ "sidebar.width": 330 }, 4));
    expect(settings.sidebarWidth).toBe(250);
    await emit("settings-state", withValues({ "sidebar.width": 330 }, 5));
    expect(settings.sidebarWidth).toBe(330);
  });

  it("reports a failed read only while no state is known", async () => {
    mockNative(() => Promise.reject(new Error("config dir unreadable")));
    settings.connect();
    await settle();
    expect(settings.error).toBe("Cannot read settings: config dir unreadable");

    await emit("settings-state", settingsState({ revision: 2 }));
    expect(settings.error).toBeNull();
  });

  it("ignores a failed read after a pushed state", async () => {
    mockNative(() => {
      void emit("settings-state", settingsState({ revision: 2 }));
      return Promise.reject(new Error("late"));
    });
    settings.connect();
    await settle();
    expect(settings.error).toBeNull();
  });

  it("reports a failed subscription without reading", async () => {
    const commands: string[] = [];
    mockIPC((cmd) => {
      commands.push(cmd);
      return cmd === "plugin:event|listen" ? Promise.reject(new Error("no event plugin")) : settingsState();
    });
    settings.connect();
    await settle();
    expect(settings.error).toBe("Cannot subscribe to settings: no event plugin");
    expect(commands).toEqual(["plugin:event|listen"]);
  });

  it("releases a subscription that completes after cleanup and drops nothing into state", async () => {
    const listen = deferred<number>();
    const commands: string[] = [];
    mockIPC((cmd) => {
      commands.push(cmd);
      return cmd === "plugin:event|listen" ? listen.promise : settingsState();
    });
    const cleanup = settings.connect();
    cleanup();
    listen.resolve(3);
    await settle();
    expect(commands).toEqual(["plugin:event|listen", "plugin:event|unlisten"]);
    expect(settings.state).toBeNull();
  });

  it("drops a read that completes after cleanup", async () => {
    const read = deferred<SettingsState>();
    mockNative(() => read.promise);
    const cleanup = settings.connect();
    await settle();
    cleanup();
    read.resolve(settingsState({ revision: 8 }));
    await settle();
    expect(settings.state).toBeNull();
  });

  it("retry clears the error and reads again", async () => {
    let fail = true;
    const commands = mockNative(() => (fail ? Promise.reject(new Error("busy")) : settingsState({ revision: 2 })));
    settings.connect();
    await settle();
    expect(settings.error).not.toBeNull();

    fail = false;
    settings.retry();
    expect(settings.error).toBeNull();
    await settle();
    expect(settings.state?.revision).toBe(2);
    expect(commands).toEqual(["settings_state", "settings_state"]);
  });
});
