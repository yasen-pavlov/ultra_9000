// The Open palette's Documents section (board 33), as pure rules: which recent Markdown files
// show, in what order, how many before “Show all N”, and when a typed path offers “View”.
import type { DocumentEntry, DocumentList } from "../native";
import { joinPath } from "./links";

/** Rows shown before “Show all N” expands the section in place. */
export const DOCUMENTS_COLLAPSED = 5;

export interface DocumentRow {
  entry: DocumentEntry;
  projectId: string;
}

export interface DocumentSection {
  /** False: the section is not drawn (global palette with an empty query). */
  visible: boolean;
  rows: DocumentRow[];
  /** Matching files, shown or not (“5 of 38”). */
  total: number;
  /** A scan stopped at its bound: the header says “recent files only”. */
  truncated: boolean;
}

/** Search matches the file name and its project-relative path, never contents. */
export function matchesDocument(entry: DocumentEntry, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") return true;
  const relative = entry.dir === "." ? entry.name : `${entry.dir}/${entry.name}`;
  return entry.name.toLowerCase().includes(needle) || relative.toLowerCase().includes(needle);
}

/**
 * Scoped palette: always shown, newest first, five rows until expanded. Global palette: shown only
 * once the query is non-empty, merged across projects, newest first.
 */
export function documentSection(
  lists: readonly { projectId: string; list: DocumentList }[],
  options: { scoped: boolean; query: string; expanded: boolean },
): DocumentSection {
  const visible = options.scoped || options.query.trim() !== "";
  const matched = lists
    .flatMap(({ projectId, list }) =>
      list.entries.filter((entry) => matchesDocument(entry, options.query)).map((entry) => ({ entry, projectId })),
    )
    .sort((a, b) => b.entry.modifiedMs - a.entry.modifiedMs);
  return {
    visible: visible && matched.length > 0,
    rows: options.expanded ? matched : matched.slice(0, DOCUMENTS_COLLAPSED),
    total: matched.length,
    truncated: lists.some(({ list }) => list.truncated),
  };
}

/**
 * A query that is a path to a Markdown file (“docs/plan.md”, “~/notes/x.markdown”) offers “View”:
 * absolute, home-relative, or relative to `base` (the scoped project, else the active one).
 */
export function typedDocumentPath(query: string, base: string, home: string): string | null {
  const text = query.trim();
  if (text === "" || /\s/.test(text) || !/\.(md|markdown)$/i.test(text)) return null;
  if (text.startsWith("/")) return joinPath("/", text);
  if (text === "~" || text.startsWith("~/")) return joinPath(home, text.slice(2));
  return joinPath(base, text);
}
