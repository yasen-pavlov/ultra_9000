// The bottom-right line, the document surface's only chrome (board 32). It says what happened in
// one quiet line and disappears when the file is at rest.
import type { DocumentEvent } from "../native";

export const COPIED_MS = 1_500;
export const REPLACED_MS = 10_000;
/** The line fades out this long after the last write (no fade under reduced motion: it just goes). */
export const QUIET_AFTER_MS = 60_000;

export interface StatusInput {
  now: number;
  lastEvent: DocumentEvent;
  lastEventAtMs: number;
  follow: boolean;
  /** Source lines that arrived below the reader's place while follow was off. */
  linesBelow: number;
  /** Outcome of the last “replaced” re-render. */
  replaced: { at: number; kept: boolean } | null;
  /** When `y` copied the path (or a heading link was copied). */
  copiedAt: number | null;
}

export interface StatusLine {
  text: string;
  /** False once the file has been quiet for a minute; the text stays so the line can fade. */
  visible: boolean;
  /** Clicking the line toggles follow; only offered while it says something about following. */
  clickable: boolean;
}

/** “2 s ago” in whole seconds, then “3 min ago”. */
export function ago(at: number, now: number): string {
  const elapsed = Math.max(0, now - at);
  if (elapsed < 60_000) return `${Math.max(1, Math.floor(elapsed / 1000))} s ago`;
  return `${Math.floor(elapsed / 60_000)} min ago`;
}

export function statusLine(input: StatusInput): StatusLine | null {
  const { now, lastEvent, lastEventAtMs, follow, linesBelow, replaced, copiedAt } = input;
  if (copiedAt !== null && now - copiedAt < COPIED_MS) return { text: "copied", visible: true, clickable: false };
  const written =
    (lastEvent === "updated" || lastEvent === "replaced" || lastEvent === "returned") && lastEventAtMs > 0;
  if (follow) {
    const text = written ? `updated ${ago(lastEventAtMs, now)} · following` : "following";
    return { text, visible: true, clickable: true };
  }
  if (linesBelow > 0) {
    const lines = linesBelow === 1 ? "1 line" : `${linesBelow} lines`;
    return { text: `+${lines} below · f to follow`, visible: true, clickable: true };
  }
  if (replaced !== null && now - replaced.at < REPLACED_MS) {
    const outcome = replaced.kept ? "kept your place" : "back to top";
    return { text: `replaced ${ago(replaced.at, now)} · ${outcome}`, visible: true, clickable: false };
  }
  if (!written) return null;
  return {
    text: `updated ${ago(lastEventAtMs, now)}`,
    visible: now - lastEventAtMs < QUIET_AFTER_MS,
    clickable: false,
  };
}
