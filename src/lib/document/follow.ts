// Follow mode (board 32): the reader sticks to the end while the file grows. Any upward move turns
// it off at once; `f`, a click on the bottom-right line, `G`/End while live, or reaching the end
// while the file is live turn it on.
import type { DocumentAction } from "./keys";

export interface ScrollFacts {
  follow: boolean;
  /** The view moved up since the last scroll event. */
  movedUp: boolean;
  atEnd: boolean;
  /** Written within the last 5 s (`liveUntilMs` in the future). */
  live: boolean;
  /** The reader moved the view itself (restoring a place, following the end). */
  programmatic: boolean;
}

export function followAfterScroll(facts: ScrollFacts): boolean {
  if (facts.programmatic) return facts.follow;
  if (facts.movedUp) return false;
  if (facts.atEnd && facts.live) return true;
  return facts.follow;
}

export function followAfterAction(follow: boolean, action: DocumentAction, live: boolean): boolean {
  switch (action.kind) {
    case "follow":
      return !follow;
    case "end":
      return live || follow;
    case "top":
      return false;
    case "scroll":
    case "heading":
      return action.direction < 0 ? false : follow;
    default:
      return follow;
  }
}

/**
 * After a reading key moved the view down (j, ↓, d, Space, PageDown, G): reaching the end while the
 * file is live turns follow on, as reaching it any other way does. Restoring a place after a
 * re-render is not a reading key and never comes here.
 */
export function followAfterKeyScroll(follow: boolean, direction: 1 | -1, atEnd: boolean, live: boolean): boolean {
  return direction === 1 && atEnd && live ? true : follow;
}
