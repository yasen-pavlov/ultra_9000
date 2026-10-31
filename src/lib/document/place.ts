// The reader's place across re-renders (board 32): the nearest heading above the top of the view
// plus an offset in pixels, not a raw scroll position, so a rewrite that moves text above the
// heading still lands on the same words. A place before the first heading is an offset from the top.

export interface Place {
  /** DOM id of the heading at or above the view's top; null = above the first heading. */
  id: string | null;
  /** Pixels from that heading's top (or the page top) to the view's top. */
  offset: number;
}

export interface HeadingTop {
  id: string;
  /** Offset of the heading's top within the scrolled content. */
  top: number;
}

/** Headings must be in document order. */
export function capturePlace(headings: readonly HeadingTop[], scrollTop: number): Place {
  let anchor: HeadingTop | null = null;
  for (const heading of headings) {
    if (heading.top > scrollTop + 1) break;
    anchor = heading;
  }
  return anchor === null ? { id: null, offset: scrollTop } : { id: anchor.id, offset: scrollTop - anchor.top };
}

/**
 * Where to scroll after a re-render. `kept` is false only when the place's heading is gone, in
 * which case the reader goes to the top and the bottom-right line says “back to top”.
 */
export function restorePlace(place: Place, headings: readonly HeadingTop[]): { scrollTop: number; kept: boolean } {
  if (place.id === null) return { scrollTop: place.offset, kept: true };
  const heading = headings.find((candidate) => candidate.id === place.id);
  return heading === undefined ? { scrollTop: 0, kept: false } : { scrollTop: heading.top + place.offset, kept: true };
}

/** Heading jumps (`[`, `]`, the outline) leave the heading this far below the view's top. */
export const HEADING_OFFSET = 8;

/** The heading the reader is at: the last one at or above the view's top plus the jump offset; -1 = none. */
export function currentHeadingIndex(headings: readonly HeadingTop[], scrollTop: number): number {
  let current = -1;
  headings.forEach((heading, index) => {
    if (heading.top <= scrollTop + HEADING_OFFSET + 1) current = index;
  });
  return current;
}

/**
 * The heading `[` / `]` goes to. `cursor` is the heading the previous press went to while its
 * scroll may still be in flight, so `]]]` moves three headings; without it the view decides. `[`
 * inside a section goes to that section's heading first.
 */
export function headingStep(
  headings: readonly HeadingTop[],
  scrollTop: number,
  cursor: number | null,
  direction: 1 | -1,
): number | null {
  let index: number;
  if (cursor !== null) index = cursor + direction;
  else {
    const current = currentHeadingIndex(headings, scrollTop);
    const inside = current >= 0 && headings[current].top < scrollTop + HEADING_OFFSET - 2;
    if (direction === 1) index = current + 1;
    else if (inside) index = current;
    else index = current - 1;
  }
  return index >= 0 && index < headings.length ? index : null;
}
