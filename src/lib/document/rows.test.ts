import { describe, expect, it } from "vitest";
import { documentItem, itemState } from "../../test/fixtures";
import { formatBytes } from "../format";
import { DOCUMENT_TOOL, toolMark } from "../icons";
import { itemView, quitLists, rowLabel } from "../items";

const NOW = 10_000_000;
const context = { tool: undefined, now: NOW, home: "/home/ana" };

describe("document rows (board 30)", () => {
  it("read file name, directory and age, with no badge, pill or state colour", () => {
    const view = itemView(documentItem({}, { modifiedMs: NOW - 2 * 60_000 }), context);
    expect(view).toMatchObject({
      title: "importer-rollover.md",
      badge: "",
      pill: "",
      rowClass: "",
      sub: "docs/reviews · 2m",
    });
    expect(view.doc).toEqual({ dir: "docs/reviews", tail: "2m", live: false });
    expect(rowLabel(view, "", "atlas", "document")).toBe(
      "importer-rollover.md, docs/reviews, atlas, document, modified 2 min ago",
    );
  });

  it("say “live” until five seconds after the last write", () => {
    const view = itemView(documentItem({}, { liveUntilMs: NOW + 1 }), context);
    expect(view.doc).toEqual({ dir: "docs/reviews", tail: "live", live: true });
    expect(itemView(documentItem({}, { liveUntilMs: NOW }), context).doc?.live).toBe(false);
  });

  it("dim and say “moved or deleted” once the path is gone", () => {
    const view = itemView(documentItem({}, { status: "missing" }), context);
    expect(view.rowClass).toBe("gone");
    expect(view.sub).toBe("docs/reviews · moved or deleted");
  });

  it("draw the neutral document mark, never a configured tool's", () => {
    const mark = toolMark(DOCUMENT_TOOL, undefined, 14, true);
    expect(mark).toMatchObject({ kind: "svg", neutral: true });
    expect(mark.kind === "svg" && mark.svg.includes("evenodd")).toBe(true);
  });
});

describe("quit sheet lists", () => {
  it("lists documents apart and never counts them as ending", () => {
    const agent = itemState({ id: "a" });
    const doc = documentItem({ id: "d" });
    expect(quitLists(["a", "d"], [agent, doc])).toEqual({ ending: [agent], documents: [doc] });
    expect(quitLists([], [doc])).toEqual({ ending: [], documents: [doc] });
  });
});

describe("formatBytes", () => {
  it("writes sizes as the boards do", () => {
    expect(formatBytes(640)).toBe("640 B");
    expect(formatBytes(18 * 1024)).toBe("18 KB");
    expect(formatBytes(14.2 * 1024 * 1024)).toBe("14.2 MB");
  });
});
