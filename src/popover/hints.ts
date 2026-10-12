// The popover footer's key hints (board 05): one 34 px band, so a row that doesn't fit never wraps
// or clips; trailing hints are dropped instead, most important first.

export interface Hint {
  /** Keycaps, in order. */
  keys: string[];
  label: string;
}

export const LIST_HINTS: readonly Hint[] = [
  { keys: ["↵"], label: "show" },
  { keys: ["j"], label: "jump" },
  { keys: ["n"], label: "new" },
  { keys: ["w"], label: "close" },
  { keys: ["1–9"], label: "row" },
];

export const EMPTY_HINTS: readonly Hint[] = [
  { keys: ["n"], label: "agent" },
  { keys: ["t"], label: "terminal" },
  { keys: ["e"], label: "Neovim" },
  { keys: ["o"], label: "palette" },
  { keys: ["esc"], label: "close" },
];

export const QUESTION_HINTS: readonly Hint[] = [
  { keys: ["w"], label: "end" },
  { keys: ["↑", "↓"], label: "keep" },
  { keys: ["esc"], label: "keep running" },
];

/** How many leading hints of these widths fit in `available` px with `gap` px between them. */
export function fittingHints(widths: readonly number[], gap: number, available: number): number {
  let used = 0;
  for (const [index, width] of widths.entries()) {
    used += width + (index > 0 ? gap : 0);
    if (used > available) return index;
  }
  return widths.length;
}
