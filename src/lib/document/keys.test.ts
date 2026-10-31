import { describe, expect, it } from "vitest";
import { documentKey, outlineMode, type KeyContext, type KeyInput } from "./keys";

function key(text: string, mods: Partial<KeyInput> = {}): KeyInput {
  return { key: text, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods };
}

const reading: KeyContext = { overlay: false, pendingG: false, hasFind: false, mac: false };

describe("reading keys", () => {
  it("maps plain keys to reading actions", () => {
    expect(documentKey(key("j"), reading).action).toEqual({ kind: "scroll", unit: "lines", direction: 1 });
    expect(documentKey(key("ArrowUp"), reading).action).toEqual({ kind: "scroll", unit: "lines", direction: -1 });
    expect(documentKey(key("u"), reading).action).toEqual({ kind: "scroll", unit: "half", direction: -1 });
    expect(documentKey(key(" ", { shiftKey: true }), reading).action).toEqual({
      kind: "scroll",
      unit: "page",
      direction: -1,
    });
    expect(documentKey(key("G", { shiftKey: true }), reading).action).toEqual({ kind: "end" });
    expect(documentKey(key("]"), reading).action).toEqual({ kind: "heading", direction: 1 });
    expect(documentKey(key("."), reading).action).toEqual({ kind: "menu" });
  });

  it("needs g twice for the top; anything else in between cancels it", () => {
    const first = documentKey(key("g"), reading);
    expect(first).toEqual({ action: null, pendingG: true });
    expect(documentKey(key("g"), { ...reading, pendingG: true }).action).toEqual({ kind: "top" });
    expect(documentKey(key("j"), { ...reading, pendingG: true })).toEqual({
      action: { kind: "scroll", unit: "lines", direction: 1 },
      pendingG: false,
    });
  });

  it("leaves app chords and other modified keys to native", () => {
    expect(documentKey(key("D", { ctrlKey: true, shiftKey: true }), reading).action).toBeNull();
    expect(documentKey(key("j", { altKey: true }), reading).action).toBeNull();
  });

  it("opens find with Ctrl+F, or Cmd+F on macOS", () => {
    expect(documentKey(key("f", { ctrlKey: true }), reading).action).toEqual({ kind: "find" });
    expect(documentKey(key("f", { metaKey: true }), reading).action).toBeNull();
    expect(documentKey(key("f", { metaKey: true }), { ...reading, mac: true }).action).toEqual({ kind: "find" });
  });

  it("steps through matches with n / N only while find has a query", () => {
    expect(documentKey(key("n"), reading).action).toBeNull();
    expect(documentKey(key("N", { shiftKey: true }), { ...reading, hasFind: true }).action).toEqual({
      kind: "find-step",
      direction: -1,
    });
  });
});

describe("keymap precedence", () => {
  const overlay: KeyContext = { ...reading, overlay: true, hasFind: true };

  it("makes plain keys inert while find, a menu or the outline holds focus", () => {
    for (const text of ["j", "k", "g", "G", "f", "o", "e", "y", "r", ".", "/", "n", " ", "Escape"]) {
      expect(documentKey(key(text), overlay)).toEqual({ action: null, pendingG: false });
    }
  });

  it("still lets Ctrl+F reach find from inside an overlay", () => {
    expect(documentKey(key("f", { ctrlKey: true }), overlay).action).toEqual({ kind: "find" });
  });
});

describe("outline placement", () => {
  it("is a 216 px rail only on a panel at least 872 px wide with three or more headings", () => {
    expect(outlineMode(872, 3, false)).toBe("rail");
    expect(outlineMode(871, 3, false)).toBe("popover");
    expect(outlineMode(1200, 2, false)).toBe("popover");
  });

  it("falls back to the popover once the rail is hidden, and has nothing without headings", () => {
    expect(outlineMode(1200, 6, true)).toBe("popover");
    expect(outlineMode(1200, 0, false)).toBe("none");
  });
});
