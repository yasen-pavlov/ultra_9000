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
 * A query that is a path to a Markdown file (“docs/plan.md”, “~/notes/x.markdown”, “./a.md”)
 * offers “View”: absolute, home-relative, or relative to `base` (the scoped project, else the
 * active one). A bare name like “report.md” is a search, not a path.
 */
export function typedDocumentPath(query: string, base: string, home: string): string | null {
  const text = query.trim();
  if (text === "" || /\s/.test(text) || !/\.(md|markdown)$/i.test(text)) return null;
  if (!text.includes("/") && !text.startsWith("~") && !text.startsWith(".")) return null;
  if (text.startsWith("/")) return joinPath("/", text);
  if (text.startsWith("~/")) return joinPath(home, text.slice(2));
  if (text.startsWith("~")) return null;
  return joinPath(base, text);
}

/** The palette row kinds these rules need to know about. */
export type PaletteEntryKind =
  "launch" | "lastset" | "lastset-dismiss" | "run" | "doc-path" | "doc" | "doc-more" | "conv";

/**
 * The cursor before the user moves it. With a query: the first real match (running, document or
 * saved), then a typed path, then the launch line. Opened by the documents chord: the newest
 * document. Opened for "open": the previous item, so ↵ toggles back.
 */
export function defaultEntry(
  entries: readonly { key: string; kind: PaletteEntryKind }[],
  options: { query: string; focus: "documents" | undefined; previousKey: string | null },
): string {
  if (options.query.trim() !== "") {
    const match = entries.find((entry) => entry.kind === "run" || entry.kind === "doc" || entry.kind === "conv");
    return match?.key ?? entries.find((entry) => entry.kind === "doc-path")?.key ?? "launch";
  }
  if (options.focus === "documents") return entries.find((entry) => entry.kind === "doc")?.key ?? "launch";
  if (options.previousKey !== null && entries.some((entry) => entry.key === options.previousKey)) {
    return options.previousKey;
  }
  return "launch";
}

/** What Ctrl+↵ opens in Neovim: an open document row, a recent file or a typed path; null = start new. */
export function editorTarget(
  entry:
    | { kind: "run"; item: { id: string; kind: string } }
    | { kind: "doc"; row: DocumentRow }
    | { kind: "doc-path"; path: string }
    | { kind: Exclude<PaletteEntryKind, "run" | "doc" | "doc-path"> }
    | null,
): { itemId: string } | { path: string } | null {
  switch (entry?.kind) {
    case "run":
      return entry.item.kind === "document" ? { itemId: entry.item.id } : null;
    case "doc":
      return entry.row.entry.openItemId === null
        ? { path: entry.row.entry.path }
        : { itemId: entry.row.entry.openItemId };
    case "doc-path":
      return { path: entry.path };
    default:
      return null;
  }
}

/**
 * Projects whose recent Markdown the palette asks native for: the scoped project only; in the
 * global palette, the sidebar's projects, and only once something is typed.
 */
export function documentProjects(scope: string | null, queryEmpty: boolean, projectIds: readonly string[]): string[] {
  if (scope !== null) return [scope];
  return queryEmpty ? [] : [...projectIds];
}
