// Component-level behaviour of the document surface: what it fetches and shows, how it routes
// links and its own controls, and where keyboard focus goes. jsdom has no layout, so every element
// reports zero size: the page is always "at its end".
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { documentState, settle } from "../../test/fixtures";
import { documentSessions } from "../document/session.svelte";
import type { DocumentState } from "../native";
import DocumentSurface from "./DocumentSurface.svelte";

interface Props {
  itemId: string;
  title: string;
  doc: DocumentState;
  presentationEpoch: number;
}

let texts: Record<number, string>;
let served: number;
let calls: { cmd: string; args: unknown }[];
let writeText: ReturnType<typeof vi.fn>;
let mounted: Record<string, unknown> | null = null;
let target: HTMLElement;

beforeEach(() => {
  texts = {};
  served = 1;
  calls = [];
  documentSessions.retain(new Set());
  vi.stubGlobal("matchMedia", () => ({ matches: false }));
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe(): void {}
      unobserve(): void {}
      disconnect(): void {}
    },
  );
  vi.stubGlobal("requestAnimationFrame", (callback: () => void) => setTimeout(callback, 0));
  Element.prototype.scrollTo = function scrollTo(this: Element, options?: ScrollToOptions | number): void {
    if (typeof options === "object") this.scrollTop = options.top ?? this.scrollTop;
  };
  writeText = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "document_text") return { revision: served, text: texts[served] ?? "" };
    return null;
  });
  target = document.createElement("div");
  document.body.append(target);
});

afterEach(() => {
  if (mounted !== null) void unmount(mounted);
  mounted = null;
  target.remove();
  clearMocks();
});

function show(props: Props): void {
  mounted = mount(DocumentSurface, { target, props });
  flushSync();
}

function hide(): void {
  if (mounted !== null) void unmount(mounted);
  mounted = null;
}

/**
 * Waits for the lazy renderer (a dynamic import plus unified's async run): real asynchronous work,
 * not a timer, so fake timers cannot drive it; polls until the condition holds.
 */
async function until(condition: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    flushSync();
    if (condition()) return;
    await settle();
  }
  throw new Error("condition not reached");
}

const prose = (): HTMLElement => target.querySelector<HTMLElement>("article.prose") as HTMLElement;
const reader = (): HTMLElement => target.querySelector<HTMLElement>(".reader") as HTMLElement;
const textCalls = (): number => calls.filter((call) => call.cmd === "document_text").length;

function key(element: Element, text: string): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { key: text, bubbles: true, cancelable: true });
  element.dispatchEvent(event);
  flushSync();
  return event;
}

function click(element: Element): MouseEvent {
  const event = new MouseEvent("click", { bubbles: true, cancelable: true });
  element.dispatchEvent(event);
  return event;
}

describe("what the surface shows", () => {
  it("renders the revision native read, and decorates identical HTML exactly once per revision", async () => {
    texts[1] = "---\nstatus: draft\n---\n# Title\n\n```js\nlet a = 1;\n```";
    const props: Props = $state({
      itemId: "d1",
      title: "a.md",
      doc: documentState({ revision: 1 }),
      presentationEpoch: 1,
    });
    show(props);
    await until(() => prose().querySelector("h1") !== null);
    // Only the frontmatter changed: the prose HTML is identical.
    texts[2] = "---\nstatus: final\n---\n# Title\n\n```js\nlet a = 1;\n```";
    served = 2;
    props.doc = documentState({ revision: 2 });
    await until(() => target.querySelector(".fm-line")?.textContent?.includes("final") === true);
    expect(prose().querySelectorAll(".h-anchor")).toHaveLength(1);
    expect(prose().querySelectorAll(".code-copy")).toHaveLength(1);
    expect(textCalls()).toBe(2);
  });

  it("keeps the last version readable under the missing notice after switching away and back", async () => {
    texts[1] = "# Report\n\nThe last version read.";
    show({ itemId: "d1", title: "a.md", doc: documentState({ revision: 1 }), presentationEpoch: 1 });
    await until(() => prose().textContent?.includes("The last version read.") === true);
    hide(); // a sheet opened over it
    show({ itemId: "d1", title: "a.md", doc: documentState({ revision: 1, status: "missing" }), presentationEpoch: 2 });
    await until(() => prose().textContent?.includes("The last version read.") === true);
    expect(target.querySelector(".notice")?.textContent).toContain("No longer at this path");
    expect(textCalls()).toBe(1);
  });
});

describe("links and the reader's own controls", () => {
  it("routes links in the footnote preview through the same dispatcher as the prose", async () => {
    texts[1] = "Claim[^1]\n\n[^1]: See [the source](https://example.com/source).";
    show({ itemId: "d1", title: "a.md", doc: documentState({ revision: 1 }), presentationEpoch: 1 });
    await until(() => prose().querySelector("a[data-footnote-ref]") !== null);
    prose()
      .querySelector("a[data-footnote-ref]")
      ?.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    flushSync();
    const preview = target.querySelector(".fn-pop a") as HTMLElement;
    expect(preview.getAttribute("href")).toBe("https://example.com/source");
    const event = click(preview);
    expect(event.defaultPrevented).toBe(true);
    await until(() => calls.some((call) => call.cmd === "document_open_link"));
    expect(calls.find((call) => call.cmd === "document_open_link")?.args).toEqual({
      itemId: "d1",
      url: "https://example.com/source",
    });
  });

  it("acts on its controls by identity and says “copied” only after the clipboard took the text", async () => {
    texts[1] = "## Root cause\n\n```sh\ncargo test\n```";
    show({ itemId: "d1", title: "a.md", doc: documentState({ revision: 1 }), presentationEpoch: 1 });
    await until(() => prose().querySelector(".code-copy") !== null);
    const copy = prose().querySelector(".code-copy") as HTMLButtonElement;
    expect(copy.tabIndex).toBe(0);
    // Enter and Space are left to the focused button, which activates it.
    expect(key(copy, "Enter").defaultPrevented).toBe(false);
    expect(key(copy, " ").defaultPrevented).toBe(false);
    click(copy);
    await until(() => target.querySelector(".line")?.textContent === "copied");
    expect(writeText).toHaveBeenCalledWith("cargo test");
    click(prose().querySelector(".h-anchor") as HTMLElement);
    await until(() => writeText.mock.calls.length === 2);
    expect(writeText).toHaveBeenLastCalledWith("/home/ana/w/atlas/docs/reviews/importer-rollover.md#root-cause");
  });

  it("never treats markup that looks like a reader control as one, and never lets a link navigate", async () => {
    texts[1] = "# Title";
    show({ itemId: "d1", title: "a.md", doc: documentState({ revision: 1 }), presentationEpoch: 1 });
    await until(() => prose().querySelector("h1") !== null);
    // Markup a diagram might carry: a same-origin link around something classed like a control.
    prose().insertAdjacentHTML(
      "beforeend",
      '<a href="/index.html"><span class="h-anchor code-copy" tabindex="0">continue</span></a>',
    );
    const spoof = prose().querySelector("a span") as HTMLElement;
    expect(click(spoof).defaultPrevented).toBe(true);
    expect(writeText).not.toHaveBeenCalled();
    // Reading keys still work with focus on it: it is part of the page, not an overlay.
    expect(key(spoof, "j").defaultPrevented).toBe(true);
  });

  it("does not claim “copied” when the clipboard refuses", async () => {
    writeText.mockRejectedValue(new Error("denied"));
    texts[1] = "# Title";
    show({ itemId: "d1", title: "a.md", doc: documentState({ revision: 1 }), presentationEpoch: 1 });
    await until(() => prose().querySelector("h1") !== null);
    key(reader(), "y");
    await until(() => writeText.mock.calls.length > 0);
    await settle();
    flushSync();
    expect(target.querySelector(".line")?.textContent ?? "").not.toBe("copied");
  });
});

describe("keyboard", () => {
  it("takes focus on intentional presentation, not on content revisions", async () => {
    texts[1] = "# One";
    const props: Props = $state({
      itemId: "d1",
      title: "a.md",
      doc: documentState({ revision: 1 }),
      presentationEpoch: 1,
    });
    show(props);
    await until(() => prose().querySelector("h1") !== null);
    expect(document.activeElement).toBe(reader());
    const outside = document.createElement("button");
    document.body.append(outside);
    outside.focus();
    texts[2] = "# Two";
    served = 2;
    props.doc = documentState({ revision: 2 });
    await until(() => prose().textContent?.includes("Two") === true);
    expect(document.activeElement).toBe(outside);
    props.presentationEpoch = 2; // the user selected this document again
    flushSync();
    expect(document.activeElement).toBe(reader());
    outside.remove();
  });

  it("closes find with Esc from the page, before anything else, and gives the page the keyboard", async () => {
    texts[1] = "# Title\n\nalpha beta";
    show({ itemId: "d1", title: "a.md", doc: documentState({ revision: 1 }), presentationEpoch: 1 });
    await until(() => prose().querySelector("h1") !== null);
    key(reader(), "/");
    expect(target.querySelector(".find")).not.toBeNull();
    reader().focus();
    key(reader(), "Escape");
    expect(target.querySelector(".find")).toBeNull();
    expect(document.activeElement).toBe(reader());
  });

  it("follows a live file after a reading key reaches its end, and not after a re-render", async () => {
    texts[1] = "# Log\n\nrun 1";
    const props: Props = $state({
      itemId: "d1",
      title: "a.md",
      doc: documentState({
        revision: 1,
        liveUntilMs: Date.now() + 60_000,
        lastEvent: "updated",
        lastEventAtMs: Date.now(),
      }),
      presentationEpoch: 1,
    });
    show(props);
    await until(() => prose().querySelector("h1") !== null);
    expect(target.querySelector(".line")?.textContent ?? "").not.toContain("following");
    key(reader(), "j");
    expect(target.querySelector(".line")?.textContent).toContain("following");
  });
});
