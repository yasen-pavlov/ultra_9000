// Find in document (board 34): case-insensitive matches in the rendered text, wrapped in <mark>
// so they can be highlighted (#5e4c22) and stepped through. Matches never span element
// boundaries; the reader's own controls (anchors, copy buttons) are skipped.

const HIT = "find-hit";
const SKIP = "[data-reader-chrome], .katex-mathml, svg";

/** Removes previous marks, restoring the original text nodes. */
export function clearMatches(root: HTMLElement): void {
  const marks = root.querySelectorAll<HTMLElement>(`mark.${HIT}`);
  for (const mark of marks) {
    const parent = mark.parentNode;
    if (parent === null) continue;
    parent.replaceChild(document.createTextNode(mark.textContent ?? ""), mark);
    parent.normalize();
  }
}

/** Wraps every occurrence of `query` and returns the marks in document order. */
export function markMatches(root: HTMLElement, query: string): HTMLElement[] {
  clearMatches(root);
  const needle = query.toLowerCase();
  if (needle === "") return [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) =>
      node.parentElement?.closest(SKIP) != null ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  });
  const texts: Text[] = [];
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) texts.push(node as Text);
  const marks: HTMLElement[] = [];
  for (const text of texts) {
    const haystack = text.data.toLowerCase();
    let at = haystack.indexOf(needle);
    if (at < 0) continue;
    let current = text;
    let consumed = 0;
    while (at >= 0) {
      const match = current.splitText(at - consumed);
      const rest = match.splitText(needle.length);
      const mark = document.createElement("mark");
      mark.className = HIT;
      match.parentNode?.replaceChild(mark, match);
      mark.appendChild(match);
      marks.push(mark);
      consumed = at + needle.length;
      current = rest;
      at = haystack.indexOf(needle, consumed);
    }
  }
  return marks;
}

/** Index of the first match at or after the view's top, so find starts where the reader is. */
export function firstMatchFrom(tops: readonly number[], scrollTop: number): number {
  const index = tops.findIndex((top) => top >= scrollTop);
  return index < 0 ? 0 : index;
}
