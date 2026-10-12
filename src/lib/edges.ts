// Needs-you rows scrolled out of a list's view: the edge chips of the sidebar and the menu-bar
// popover. Rows keep list order; a row within `slack` px of an edge still counts as visible.

export interface Span {
  top: number;
  bottom: number;
}

export interface Offscreen {
  /** Rows above the view, in list order (the nearest is last). */
  above: string[];
  /** Rows below the view, in list order (the nearest is first). */
  below: string[];
}

export function offscreenRows(view: Span, rows: readonly (Span & { id: string })[], slack = 4): Offscreen {
  const above: string[] = [];
  const below: string[] = [];
  for (const row of rows) {
    if (row.bottom <= view.top + slack) above.push(row.id);
    else if (row.top >= view.bottom - slack) below.push(row.id);
  }
  return { above, below };
}

/** The `[data-need]` rows (SidebarRow and the collapsed strip mark needs-you items) of `list`. */
export function measureOffscreenNeeds(list: HTMLElement): Offscreen {
  const rows: (Span & { id: string })[] = [];
  for (const element of list.querySelectorAll<HTMLElement>("[data-need]")) {
    const id = element.dataset.itemId;
    if (id === undefined) continue;
    const rect = element.getBoundingClientRect();
    rows.push({ id, top: rect.top, bottom: rect.bottom });
  }
  return offscreenRows(list.getBoundingClientRect(), rows);
}

export function sameIds(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((id, index) => id === b[index]);
}

/** "↑ 1 needs you" / "↓ 2 need you". */
export function edgeChipLabel(direction: "up" | "down", count: number): string {
  return `${direction === "up" ? "↑" : "↓"} ${count} ${count === 1 ? "needs" : "need"} you`;
}
