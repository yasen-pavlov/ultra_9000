// Reading keys in a document (board 34, contract item 8). The web view owns the keyboard while a
// document is selected, so plain keys are safe here and only here. App chords are not handled:
// native routes them by physical key before the web view, exactly as it does over a sheet.

export type DocumentAction =
  | { kind: "scroll"; unit: "lines" | "half" | "page"; direction: 1 | -1 }
  | { kind: "top" }
  | { kind: "end" }
  | { kind: "heading"; direction: 1 | -1 }
  | { kind: "find-step"; direction: 1 | -1 }
  | {
      kind: "follow" | "outline" | "find" | "neovim" | "reveal" | "copy-path" | "reload" | "menu" | "escape";
    };

export interface KeyInput {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export interface KeyContext {
  /** Find, the actions menu, the outline or a popover holds focus: their own keys only. */
  overlay: boolean;
  /** A first `g` is waiting for the second. */
  pendingG: boolean;
  /** Find has a query, so `n` / `N` step through its matches. */
  hasFind: boolean;
  /** macOS: Cmd+F opens find; elsewhere Ctrl+F. */
  mac: boolean;
}

export interface KeyResult {
  action: DocumentAction | null;
  pendingG: boolean;
}

const PLAIN: Readonly<Record<string, DocumentAction>> = {
  j: { kind: "scroll", unit: "lines", direction: 1 },
  ArrowDown: { kind: "scroll", unit: "lines", direction: 1 },
  k: { kind: "scroll", unit: "lines", direction: -1 },
  ArrowUp: { kind: "scroll", unit: "lines", direction: -1 },
  d: { kind: "scroll", unit: "half", direction: 1 },
  u: { kind: "scroll", unit: "half", direction: -1 },
  " ": { kind: "scroll", unit: "page", direction: 1 },
  PageDown: { kind: "scroll", unit: "page", direction: 1 },
  PageUp: { kind: "scroll", unit: "page", direction: -1 },
  Home: { kind: "top" },
  G: { kind: "end" },
  End: { kind: "end" },
  "[": { kind: "heading", direction: -1 },
  "]": { kind: "heading", direction: 1 },
  f: { kind: "follow" },
  o: { kind: "outline" },
  "/": { kind: "find" },
  e: { kind: "neovim" },
  v: { kind: "reveal" },
  y: { kind: "copy-path" },
  r: { kind: "reload" },
  ".": { kind: "menu" },
  Escape: { kind: "escape" },
};

/** Ctrl+F (Cmd+F on macOS) opens find from anywhere in the surface. */
function findChord(input: KeyInput, mac: boolean): boolean {
  const command = mac ? input.metaKey && !input.ctrlKey : input.ctrlKey && !input.metaKey;
  return command && !input.altKey && !input.shiftKey && input.key.toLowerCase() === "f";
}

/** Plain reading keys from the page: `gg`, Shift+Space, n / N with a query, then the table. */
function readingKey(input: KeyInput, context: KeyContext): KeyResult {
  if (input.key === "g") {
    return context.pendingG ? { action: { kind: "top" }, pendingG: false } : { action: null, pendingG: true };
  }
  if (input.key === " " && input.shiftKey) {
    return { action: { kind: "scroll", unit: "page", direction: -1 }, pendingG: false };
  }
  if ((input.key === "n" || input.key === "N") && context.hasFind) {
    return { action: { kind: "find-step", direction: input.key === "n" ? 1 : -1 }, pendingG: false };
  }
  return { action: Object.hasOwn(PLAIN, input.key) ? PLAIN[input.key] : null, pendingG: false };
}

/** Maps one keydown to a reading action; null leaves the event alone (app chords, typing in overlays). */
export function documentKey(input: KeyInput, context: KeyContext): KeyResult {
  if (findChord(input, context.mac)) return { action: { kind: "find" }, pendingG: false };
  if (input.ctrlKey || input.metaKey || input.altKey) return { action: null, pendingG: false };
  // Esc is not a reading key: it reaches the surface from any control that did not handle it, so
  // the surface closes find, the menu or a popover in order (focused overlays handle their own).
  if (input.key === "Escape") return { action: { kind: "escape" }, pendingG: false };
  if (context.overlay) return { action: null, pendingG: false };
  return readingKey(input, context);
}

/** The panel and heading count decide where the outline lives (board 31). */
export const OUTLINE_RAIL_MIN_PANEL = 872;
export const OUTLINE_MIN_HEADINGS = 3;

/** “rail” beside the prose, “popover” on `o`, or “none” with no H2/H3 at all. */
export function outlineMode(panelWidth: number, entries: number, railHidden: boolean): "rail" | "popover" | "none" {
  if (entries === 0) return "none";
  if (!railHidden && panelWidth >= OUTLINE_RAIL_MIN_PANEL && entries >= OUTLINE_MIN_HEADINGS) return "rail";
  return "popover";
}
