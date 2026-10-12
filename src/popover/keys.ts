// Popover keys (board 06): the app's macOS ⌘ chords without ⌘, read from physical keys
// (`KeyboardEvent.code`, like the app's own shortcut routing) so they work on any layout.
// Modified keys pass through, so ⌘Q in the popover still reaches the app.

export type NewKind = "agent" | "shell" | "neovim";

export type PopoverKey =
  | { kind: "move"; to: "up" | "down" | "home" | "end" }
  | { kind: "enter" }
  /** 1–9: the nth row in list order (0-based). */
  | { kind: "row"; index: number }
  | { kind: "jump" }
  | { kind: "palette" }
  | { kind: "window" }
  | { kind: "new"; item: NewKind }
  | { kind: "close" }
  | { kind: "escape" };

export interface KeyInput {
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

const FIXED: Readonly<Record<string, PopoverKey>> = {
  ArrowUp: { kind: "move", to: "up" },
  ArrowDown: { kind: "move", to: "down" },
  Home: { kind: "move", to: "home" },
  End: { kind: "move", to: "end" },
  Enter: { kind: "enter" },
  NumpadEnter: { kind: "enter" },
  KeyJ: { kind: "jump" },
  KeyO: { kind: "palette" },
  KeyH: { kind: "window" },
  KeyN: { kind: "new", item: "agent" },
  KeyT: { kind: "new", item: "shell" },
  KeyE: { kind: "new", item: "neovim" },
  KeyW: { kind: "close" },
  // ⌫ on a Mac keyboard is Backspace; forward delete (fn ⌫) is Delete.
  Backspace: { kind: "close" },
  Delete: { kind: "close" },
  Escape: { kind: "escape" },
};

export function popoverKey(event: KeyInput): PopoverKey | null {
  if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return null;
  const digit = /^(?:Digit|Numpad)([1-9])$/.exec(event.code);
  if (digit !== null) return { kind: "row", index: Number(digit[1]) - 1 };
  return FIXED[event.code] ?? null;
}
