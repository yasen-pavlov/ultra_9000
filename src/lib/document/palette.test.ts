import { describe, expect, it } from "vitest";
import type { DocumentEntry, DocumentList } from "../native";
import {
  DOCUMENTS_COLLAPSED,
  defaultEntry,
  documentProjects,
  documentSection,
  editorTarget,
  matchesDocument,
  typedDocumentPath,
} from "./palette";

function entry(name: string, dir: string, modifiedMs: number): DocumentEntry {
  return { path: `/w/atlas/${dir}/${name}`, name, dir, modifiedMs, sizeBytes: 100, openItemId: null };
}

function list(entries: DocumentEntry[], truncated = false): DocumentList {
  return { entries, truncated, scannedAtMs: 0 };
}

const atlas = list(Array.from({ length: 8 }, (_, index) => entry(`note-${index}.md`, "docs", 1000 - index)));

describe("the palette's Documents section", () => {
  it("shows five rows of the scoped project's newest files until expanded", () => {
    const section = documentSection([{ projectId: "atlas", list: atlas }], {
      scoped: true,
      query: "",
      expanded: false,
    });
    expect(section.visible).toBe(true);
    expect(section.rows).toHaveLength(DOCUMENTS_COLLAPSED);
    expect(section.total).toBe(8);
    expect(section.rows[0].entry.name).toBe("note-0.md");
    const all = documentSection([{ projectId: "atlas", list: atlas }], { scoped: true, query: "", expanded: true });
    expect(all.rows).toHaveLength(8);
  });

  it("stays hidden in the global palette until something is typed, then merges projects newest first", () => {
    const lists = [
      { projectId: "atlas", list: list([entry("plan.md", ".", 10)]) },
      { projectId: "meridian", list: list([entry("plan-b.md", "docs", 20)]) },
    ];
    expect(documentSection(lists, { scoped: false, query: "", expanded: false }).visible).toBe(false);
    const found = documentSection(lists, { scoped: false, query: "plan", expanded: false });
    expect(found.visible).toBe(true);
    expect(found.rows.map((row) => [row.projectId, row.entry.name])).toEqual([
      ["meridian", "plan-b.md"],
      ["atlas", "plan.md"],
    ]);
  });

  it("matches the file name and its relative path, case-insensitively, never contents", () => {
    const report = entry("Importer-Rollover.md", "docs/reviews", 1);
    expect(matchesDocument(report, "rollover")).toBe(true);
    expect(matchesDocument(report, "reviews/imp")).toBe(true);
    expect(matchesDocument(report, "flaky")).toBe(false);
  });

  it("says “recent files only” when a scan stopped at its bound", () => {
    const section = documentSection([{ projectId: "atlas", list: list(atlas.entries, true) }], {
      scoped: true,
      query: "",
      expanded: false,
    });
    expect(section.truncated).toBe(true);
  });

  it("hides an empty section", () => {
    expect(
      documentSection([{ projectId: "atlas", list: list([]) }], { scoped: true, query: "", expanded: false }).visible,
    ).toBe(false);
  });
});

describe("a typed Markdown path offers View", () => {
  it("resolves relative paths against the project, ~ against home, and keeps absolute paths", () => {
    expect(typedDocumentPath("docs/plan.md", "/w/atlas", "/home/ana")).toBe("/w/atlas/docs/plan.md");
    expect(typedDocumentPath("~/notes/today.markdown", "/w/atlas", "/home/ana")).toBe("/home/ana/notes/today.markdown");
    expect(typedDocumentPath("/tmp/../srv/x.MD", "/w/atlas", "/home/ana")).toBe("/srv/x.MD");
  });

  it("ignores ordinary searches, including a bare file name", () => {
    expect(typedDocumentPath("importer rollover", "/w/atlas", "/home/ana")).toBeNull();
    expect(typedDocumentPath("notes.txt", "/w/atlas", "/home/ana")).toBeNull();
    expect(typedDocumentPath("report.md", "/w/atlas", "/home/ana")).toBeNull();
    expect(typedDocumentPath("./report.md", "/w/atlas", "/home/ana")).toBe("/w/atlas/report.md");
  });
});

describe("the palette's default cursor", () => {
  const entries = [
    { key: "launch", kind: "launch" as const },
    { key: "path:/w/atlas/docs/report.md", kind: "doc-path" as const },
    { key: "run:a", kind: "run" as const },
    { key: "doc:/w/atlas/docs/report.md", kind: "doc" as const },
  ];

  it("prefers a real match over a typed path", () => {
    expect(
      defaultEntry(entries.slice(0, 2).concat(entries[3]), {
        query: "docs/report.md",
        focus: undefined,
        previousKey: null,
      }),
    ).toBe("doc:/w/atlas/docs/report.md");
    expect(defaultEntry(entries.slice(0, 2), { query: "docs/report.md", focus: undefined, previousKey: null })).toBe(
      "path:/w/atlas/docs/report.md",
    );
  });

  it("starts in Documents when opened by the documents chord, else on the previous item", () => {
    expect(defaultEntry(entries, { query: "", focus: "documents", previousKey: "run:a" })).toBe(
      "doc:/w/atlas/docs/report.md",
    );
    expect(defaultEntry(entries, { query: "", focus: undefined, previousKey: "run:a" })).toBe("run:a");
    expect(defaultEntry(entries, { query: "", focus: undefined, previousKey: "run:gone" })).toBe("launch");
  });
});

describe("Ctrl+↵ on a document row opens Neovim on that file", () => {
  const recent = { projectId: "atlas", entry: entry("plan.md", "docs", 1) };

  it("uses the open row for a running document and an open recent file, the path otherwise", () => {
    expect(editorTarget({ kind: "run", item: { id: "d1", kind: "document" } })).toEqual({ itemId: "d1" });
    expect(editorTarget({ kind: "doc", row: recent })).toEqual({ path: "/w/atlas/docs/plan.md" });
    expect(editorTarget({ kind: "doc", row: { ...recent, entry: { ...recent.entry, openItemId: "d2" } } })).toEqual({
      itemId: "d2",
    });
    expect(editorTarget({ kind: "doc-path", path: "/w/atlas/x.md" })).toEqual({ path: "/w/atlas/x.md" });
  });

  it("leaves agents, terminals and other rows to “start new”", () => {
    expect(editorTarget({ kind: "run", item: { id: "a1", kind: "agent" } })).toBeNull();
    expect(editorTarget({ kind: "conv" })).toBeNull();
    expect(editorTarget(null)).toBeNull();
  });
});

describe("which projects the palette scans", () => {
  it("scans only the scoped project, and nothing globally until something is typed", () => {
    expect(documentProjects("atlas", true, ["atlas", "meridian"])).toEqual(["atlas"]);
    expect(documentProjects(null, true, ["atlas", "meridian"])).toEqual([]);
    expect(documentProjects(null, false, ["atlas", "meridian"])).toEqual(["atlas", "meridian"]);
  });
});
