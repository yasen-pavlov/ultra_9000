// When the document surface asks native for text, and which reply it shows (contract items 3–4).
// Native bumps `document.revision` after every successful read; the surface fetches only while it
// is the visible surface, and a reply never replaces a newer one already shown.
import type { DocumentState } from "../native";

/**
 * Fetch when the surface is visible and native read something newer than what was fetched. A
 * missing or unreadable file still serves its last good text (revision > 0); a too-large file has
 * none.
 */
export function wantsText(doc: Pick<DocumentState, "status" | "revision">, fetched: number, visible: boolean): boolean {
  if (!visible || doc.status === "too-large") return false;
  if (doc.status !== "ok" && doc.revision === 0) return false;
  return doc.revision > fetched;
}

/** A reply is shown only when it is newer than the text on screen, whatever order replies arrive in. */
export function acceptsText(reply: number, shown: number): boolean {
  return reply > shown;
}
