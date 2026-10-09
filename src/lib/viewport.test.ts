import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { deferred, settle, type Deferred } from "../test/fixtures";
import type { ViewportRect } from "./native";
import type * as ViewportModule from "./viewport";

type Viewport = typeof ViewportModule;

interface ViewportCall {
  itemId: string;
  presentationEpoch: number;
  rect: ViewportRect;
  visible: boolean;
  sequence: number;
}

interface FocusCall {
  itemId: string;
  presentationEpoch: number;
  sequence: number;
}

const SEQUENCE_KEY = "ultra9000.viewport.sequence";

let viewport: Viewport;
let reports: ViewportCall[];
let focuses: FocusCall[];
/** Native's answer to the next terminal_set_viewport; defaults to applied. */
let viewportReplies: (() => Promise<boolean> | boolean)[];
let focusReplies: (() => Promise<boolean> | boolean)[];
let frames: Map<number, FrameRequestCallback>;
let resizeCallbacks: ResizeObserverCallback[];
let container: HTMLDivElement;
let host: HTMLDivElement;
const trackers: ViewportModule.ViewportTracker[] = [];

/** Runs every queued animation frame callback. */
function flushFrames(): void {
  const queued = [...frames.values()];
  frames.clear();
  for (const callback of queued) callback(0);
}

/** A layout change the ResizeObserver reports. */
function observeResize(): void {
  for (const callback of resizeCallbacks) callback([], {} as ResizeObserver);
}

function place(element: HTMLElement, left: number, top: number, right: number, bottom: number): void {
  element.getBoundingClientRect = () => new DOMRect(left, top, right - left, bottom - top);
}

function reply(value: boolean | Deferred<boolean>): () => Promise<boolean> | boolean {
  return typeof value === "boolean" ? () => value : () => value.promise;
}

beforeEach(async () => {
  sessionStorage.clear();
  reports = [];
  focuses = [];
  viewportReplies = [];
  focusReplies = [];
  frames = new Map();
  resizeCallbacks = [];
  let nextFrame = 1;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    const id = nextFrame++;
    frames.set(id, callback);
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
  vi.stubGlobal(
    "ResizeObserver",
    class {
      #callback: ResizeObserverCallback;
      constructor(callback: ResizeObserverCallback) {
        this.#callback = callback;
        resizeCallbacks.push(callback);
      }
      observe(): void {}
      disconnect(): void {
        resizeCallbacks = resizeCallbacks.filter((callback) => callback !== this.#callback);
      }
    },
  );
  mockIPC((cmd, args) => {
    if (cmd === "terminal_set_viewport") {
      reports.push(args as unknown as ViewportCall);
      return (viewportReplies.shift() ?? (() => true))();
    }
    if (cmd === "terminal_focus") {
      focuses.push(args as unknown as FocusCall);
      return (focusReplies.shift() ?? (() => true))();
    }
    throw new Error(`unexpected command ${cmd}`);
  });

  document.body.replaceChildren();
  container = document.createElement("div");
  host = document.createElement("div");
  container.append(host);
  document.body.append(container);
  place(container, 300, 0, 1200, 800);
  place(host, 300, 40, 1200, 800);

  await load();
});

afterEach(() => {
  // Trackers outlive their test (window resize listeners); end them while IPC is still mocked.
  for (const tracker of trackers.splice(0)) tracker.dispose();
  clearMocks();
});

/**
 * A fresh web view load of the module: its state (sequence, last rect, active tracker) is per load,
 * which a static import cannot reset. Trackers it creates are recorded for teardown.
 */
async function load(): Promise<void> {
  vi.resetModules();
  const loaded = await import("./viewport");
  viewport = {
    ...loaded,
    trackViewport(element, target) {
      const tracker = loaded.trackViewport(element, target);
      trackers.push(tracker);
      return tracker;
    },
  };
}

const shown = (itemId: string, presentationEpoch = 1) => ({ itemId, presentationEpoch, visible: true });

describe("reporting geometry", () => {
  it("reports the host rectangle for the shown item and focuses it with that report's sequence", async () => {
    viewport.trackViewport(host, shown("item-1", 3));
    await settle();
    expect(reports).toHaveLength(1);
    expect(reports[0]).toMatchObject({
      itemId: "item-1",
      presentationEpoch: 3,
      rect: { x: 300, y: 40, width: 900, height: 760, topLeftRadius: 0, bottomLeftRadius: 0 },
      visible: true,
    });
    expect(focuses).toEqual([{ itemId: "item-1", presentationEpoch: 3, sequence: reports[0].sequence }]);
  });

  it("rounds edges rather than sizes and clamps negative positions", async () => {
    place(host, -4.6, 10.4, 310.5, 200.6);
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(reports[0].rect).toMatchObject({ x: 0, y: 10, width: 311, height: 191 });
  });

  it("coalesces layout changes into one report per animation frame and skips unchanged geometry", async () => {
    viewport.trackViewport(host, shown("item-1"));
    await settle();

    place(host, 320, 40, 1200, 800);
    observeResize();
    window.dispatchEvent(new Event("resize"));
    expect(frames.size).toBe(1);
    flushFrames();
    await settle();
    expect(reports.map((report) => report.rect.x)).toEqual([300, 320]);

    observeResize();
    flushFrames();
    await settle();
    expect(reports).toHaveLength(2);
  });

  it("issues ever-increasing sequences, never below the wall clock, and persists the last one", async () => {
    vi.spyOn(Date, "now").mockReturnValue(5_000);
    const tracker = viewport.trackViewport(host, shown("item-1"));
    tracker.setTarget(shown("item-2"));
    tracker.setTarget(shown("item-3"));
    await settle();
    expect(reports.map((report) => report.sequence)).toEqual([5_000, 5_001, 5_002]);
    expect(sessionStorage.getItem(SEQUENCE_KEY)).toBe("5002");
  });

  it("continues after the stored sequence across a web view reload even if the clock went back", async () => {
    sessionStorage.setItem(SEQUENCE_KEY, "90000");
    vi.spyOn(Date, "now").mockReturnValue(1_000);
    await load();
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(reports[0].sequence).toBe(90_001);
  });

  it("ignores a corrupt stored sequence", async () => {
    sessionStorage.setItem(SEQUENCE_KEY, "-7");
    vi.spyOn(Date, "now").mockReturnValue(2_000);
    await load();
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(reports[0].sequence).toBe(2_000);
  });

  it("stays monotonic when session storage is unavailable", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("storage disabled");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("storage disabled");
    });
    vi.spyOn(Date, "now").mockReturnValue(3_000);
    await load();
    const tracker = viewport.trackViewport(host, shown("item-1"));
    tracker.setTarget(shown("item-2"));
    await settle();
    expect(reports.map((report) => report.sequence)).toEqual([3_000, 3_001]);
  });
});

describe("hidden reports", () => {
  it("sends nothing for a zero-sized host before any usable geometry", async () => {
    place(host, 300, 40, 300, 800);
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(reports).toEqual([]);
    expect(focuses).toEqual([]);
  });

  it("hides with the last usable rect when the host collapses, so native never sizes a PTY to zero", async () => {
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    place(host, 300, 40, 300, 40);
    observeResize();
    flushFrames();
    await settle();
    expect(reports.map(({ rect, visible }) => ({ width: rect.width, height: rect.height, visible }))).toEqual([
      { width: 900, height: 760, visible: true },
      { width: 900, height: 760, visible: false },
    ]);
  });

  it("reports a hidden surface with usable geometry, shows it, and hides it again without focusing", async () => {
    const tracker = viewport.trackViewport(host, { itemId: "item-1", presentationEpoch: 1, visible: false });
    await settle();
    expect(reports.map((report) => report.visible)).toEqual([false]);
    expect(focuses).toEqual([]);

    tracker.setTarget(shown("item-1"));
    await settle();
    tracker.setTarget({ itemId: "item-1", presentationEpoch: 1, visible: false });
    await settle();
    expect(reports.map((report) => report.visible)).toEqual([false, true, false]);
    expect(reports[2].rect.width).toBe(900);
    expect(focuses).toEqual([{ itemId: "item-1", presentationEpoch: 1, sequence: reports[1].sequence }]);
  });

  it("sends nothing without a selected item and resends once an item is selected again", async () => {
    const tracker = viewport.trackViewport(host, shown("item-1"));
    await settle();
    tracker.setTarget({ itemId: null, presentationEpoch: 2, visible: true });
    await settle();
    expect(reports).toHaveLength(1);

    tracker.setTarget(shown("item-1"));
    await settle();
    expect(reports.map((report) => [report.itemId, report.visible])).toEqual([
      ["item-1", true],
      ["item-1", true],
    ]);
  });
});

describe("targets and stale replies", () => {
  it("reports a new presentation immediately and ignores an unchanged target", async () => {
    const tracker = viewport.trackViewport(host, shown("item-1", 1));
    tracker.setTarget(shown("item-1", 1));
    tracker.setTarget(shown("item-1", 2));
    await settle();
    expect(reports.map((report) => report.presentationEpoch)).toEqual([1, 2]);
    expect(frames.size).toBe(0);
    expect(focuses.map((focus) => focus.presentationEpoch)).toEqual([2]);
  });

  it("does not focus when native rejects the report, and resends the same geometry later", async () => {
    viewportReplies.push(reply(false));
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(focuses).toEqual([]);

    observeResize();
    flushFrames();
    await settle();
    expect(reports).toHaveLength(2);
    expect(focuses).toEqual([{ itemId: "item-1", presentationEpoch: 1, sequence: reports[1].sequence }]);
  });

  it("focuses with the newest report's sequence when layout changes while a report is pending", async () => {
    const first = deferred<boolean>();
    viewportReplies.push(reply(first));
    viewport.trackViewport(host, shown("item-1"));
    await settle();

    place(host, 310, 40, 1200, 800);
    observeResize();
    flushFrames();
    first.resolve(true);
    await settle();
    expect(reports).toHaveLength(2);
    expect(focuses).toEqual([{ itemId: "item-1", presentationEpoch: 1, sequence: reports[1].sequence }]);
  });

  it("never runs focus queued for an earlier presentation", async () => {
    const pending = deferred<boolean>();
    viewportReplies.push(reply(pending));
    const tracker = viewport.trackViewport(host, shown("item-1", 1));
    await settle();
    tracker.setTarget(shown("item-2", 2));
    pending.resolve(true);
    await settle();
    expect(focuses.map((focus) => focus.itemId)).toEqual(["item-2"]);
  });

  it("drops focus work cancelled while its report is in flight", async () => {
    const pending = deferred<boolean>();
    viewportReplies.push(reply(pending));
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    viewport.cancelTerminalFocus();
    pending.resolve(true);
    await settle();
    expect(focuses).toEqual([]);
  });
});

describe("errors", () => {
  it("surfaces a failed report and clears it once a report applies", async () => {
    const errors: (string | null)[] = [];
    viewport.onViewportError((error) => errors.push(error));
    viewportReplies.push(() => Promise.reject(new Error("no surface")));
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    // Adopting the new presentation first clears any stale error.
    expect(errors).toEqual([null, "Terminal viewport update failed: no surface"]);
    expect(focuses).toEqual([]);

    // The failed geometry is not suppressed as a duplicate.
    observeResize();
    flushFrames();
    await settle();
    expect(errors.at(-1)).toBeNull();
    expect(focuses).toHaveLength(1);
  });

  it("ignores the failure of a report a newer one superseded", async () => {
    const errors: (string | null)[] = [];
    viewport.onViewportError((error) => errors.push(error));
    const stale = deferred<boolean>();
    viewportReplies.push(reply(stale));
    const tracker = viewport.trackViewport(host, shown("item-1", 1));
    await settle();
    tracker.setTarget(shown("item-1", 2));
    stale.reject(new Error("stale"));
    await settle();
    expect(errors.filter((error) => error !== null)).toEqual([]);
  });

  it("clears the error when the presentation changes", async () => {
    const errors: (string | null)[] = [];
    viewport.onViewportError((error) => errors.push(error));
    const tracker = viewport.trackViewport(host, shown("item-1", 1));
    await settle();
    errors.length = 0;
    tracker.setTarget({ itemId: "item-1", presentationEpoch: 1, visible: false });
    expect(errors).toEqual([]);
    tracker.setTarget(shown("item-2", 2));
    expect(errors[0]).toBeNull();
  });

  it("surfaces a failed focus while the terminal is still shown", async () => {
    const errors: (string | null)[] = [];
    viewport.onViewportError((error) => errors.push(error));
    focusReplies.push(() => Promise.reject(new Error("window not key")));
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(errors.at(-1)).toBe("Cannot focus terminal: window not key");
  });

  it("keeps a failed focus quiet once the terminal was hidden meanwhile", async () => {
    const errors: (string | null)[] = [];
    viewport.onViewportError((error) => errors.push(error));
    const focus = deferred<boolean>();
    focusReplies.push(reply(focus));
    const tracker = viewport.trackViewport(host, shown("item-1"));
    await settle();
    tracker.setTarget({ itemId: "item-1", presentationEpoch: 1, visible: false });
    focus.reject(new Error("late"));
    await settle();
    expect(errors.filter((error) => error !== null)).toEqual([]);
  });

  it("a replaced listener's unsubscribe leaves the newer listener in place", async () => {
    const first: (string | null)[] = [];
    const second: (string | null)[] = [];
    const stopFirst = viewport.onViewportError((error) => first.push(error));
    const stopSecond = viewport.onViewportError((error) => second.push(error));
    stopFirst();
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(first).toEqual([]);
    expect(second.length).toBeGreaterThan(0);

    stopSecond();
    const delivered = second.length;
    viewport.requestTerminalFocus();
    await settle();
    expect(second).toHaveLength(delivered);
  });
});

describe("corner radii", () => {
  it("rounds the native corners the content panel rounds where the host sits flush", async () => {
    container.style.borderTopLeftRadius = "8px 12px";
    container.style.borderBottomLeftRadius = "10px";
    place(host, 300, 0, 1200, 800);
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(reports[0].rect).toMatchObject({ topLeftRadius: 8, bottomLeftRadius: 10 });
  });

  it("keeps a corner square where something sits between host and panel edge", async () => {
    container.style.borderTopLeftRadius = "8px";
    container.style.borderBottomLeftRadius = "8px";
    // An exit bar above the host: top edge not flush, bottom edge flush.
    place(host, 300, 40.4, 1200, 799.8);
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(reports[0].rect).toMatchObject({ topLeftRadius: 0, bottomLeftRadius: 8 });
  });

  it("keeps both corners square when the host is not flush left or the radius is unusable", async () => {
    container.style.borderTopLeftRadius = "8px";
    place(host, 301, 0, 1200, 800);
    const tracker = viewport.trackViewport(host, shown("item-1"));
    await settle();
    expect(reports[0].rect).toMatchObject({ topLeftRadius: 0, bottomLeftRadius: 0 });

    container.style.borderTopLeftRadius = "0px";
    place(host, 300, 0, 1200, 800);
    tracker.setTarget(shown("item-1", 2));
    await settle();
    expect(reports[1].rect).toMatchObject({ topLeftRadius: 0, bottomLeftRadius: 0 });
  });

  it("keeps corners square for a host without a container", async () => {
    const orphan = document.createElement("div");
    place(orphan, 0, 0, 100, 100);
    viewport.trackViewport(orphan, shown("item-1"));
    await settle();
    expect(reports[0].rect).toMatchObject({ topLeftRadius: 0, bottomLeftRadius: 0 });
  });
});

describe("tracker lifecycle", () => {
  it("hides the surface on dispose and stops following layout", async () => {
    const tracker = viewport.trackViewport(host, shown("item-1"));
    await settle();
    tracker.dispose();
    tracker.dispose();
    await settle();
    expect(reports.map((report) => report.visible)).toEqual([true, false]);

    place(host, 0, 0, 10, 10);
    observeResize();
    window.dispatchEvent(new Event("resize"));
    flushFrames();
    tracker.setTarget(shown("item-2", 2));
    await settle();
    expect(reports).toHaveLength(2);
  });

  it("cancels a pending frame on dispose", async () => {
    const tracker = viewport.trackViewport(host, shown("item-1"));
    window.dispatchEvent(new Event("resize"));
    expect(frames.size).toBe(1);
    tracker.dispose();
    expect(frames.size).toBe(0);
    await settle();
  });

  it("lets a late teardown of a replaced tracker leave the newer view alone", async () => {
    const old = viewport.trackViewport(host, shown("item-1"));
    await settle();
    const replacement = viewport.trackViewport(host, shown("item-1", 2));
    await settle();
    old.dispose();
    old.setTarget(shown("item-9", 9));
    await settle();
    expect(reports.map((report) => [report.presentationEpoch, report.visible])).toEqual([
      [1, true],
      [2, true],
    ]);
    replacement.dispose();
    await settle();
    expect(reports.at(-1)?.visible).toBe(false);
  });
});

describe("requestTerminalFocus", () => {
  it("re-applies the unchanged geometry with a new sequence and focuses with it", async () => {
    viewport.trackViewport(host, shown("item-1"));
    await settle();
    viewport.requestTerminalFocus();
    await settle();
    expect(reports).toHaveLength(2);
    expect(reports[1].rect).toEqual(reports[0].rect);
    expect(reports[1].sequence).toBeGreaterThan(reports[0].sequence);
    expect(focuses.map((focus) => focus.sequence)).toEqual([reports[0].sequence, reports[1].sequence]);
  });

  it("does nothing while no item is shown or nothing is tracked", async () => {
    viewport.requestTerminalFocus();
    const tracker = viewport.trackViewport(host, { itemId: "item-1", presentationEpoch: 1, visible: false });
    viewport.requestTerminalFocus();
    await settle();
    tracker.setTarget(shown("item-1"));
    await settle();
    tracker.dispose();
    await settle();
    const before = reports.length;
    viewport.requestTerminalFocus();
    await settle();
    expect(reports).toHaveLength(before);
    expect(focuses).toHaveLength(1);
  });
});
