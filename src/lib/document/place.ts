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
