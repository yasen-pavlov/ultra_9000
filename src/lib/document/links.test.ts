import { describe, expect, it } from "vitest";
import { joinPath, linkTarget } from "./links";

const DOC = "/w/atlas/docs/reviews/report.md";

describe("links in a document never navigate the web view", () => {
  it("scrolls to in-page fragments", () => {
    expect(linkTarget(DOC, "#root-cause")).toEqual({ kind: "fragment", id: "root-cause" });
    expect(linkTarget(DOC, "#")).toEqual({ kind: "none" });
  });

  it("opens http(s) and mailto outside through native", () => {
    expect(linkTarget(DOC, "https://ci.example/flakes")).toEqual({
      kind: "external",
      url: "https://ci.example/flakes",
    });
    expect(linkTarget(DOC, "mailto:ana@example.com")).toEqual({ kind: "external", url: "mailto:ana@example.com" });
  });

  it("opens relative Markdown files as documents, resolved against the document's folder", () => {
    expect(linkTarget(DOC, "../handoff%20notes.md#next")).toEqual({
      kind: "document",
      path: "/w/atlas/docs/handoff notes.md",
    });
    expect(linkTarget(DOC, "/etc/motd.markdown")).toEqual({ kind: "document", path: "/etc/motd.markdown" });
  });

  it("does nothing for other schemes and non-Markdown files", () => {
    expect(linkTarget(DOC, "javascript:alert(1)")).toEqual({ kind: "none" });
    expect(linkTarget(DOC, "file:///etc/passwd")).toEqual({ kind: "none" });
    expect(linkTarget(DOC, "../src/main.rs")).toEqual({ kind: "none" });
    expect(linkTarget(DOC, "//cdn.example/x.md")).toEqual({ kind: "none" });
  });

  it("normalises dot segments without escaping the root", () => {
    expect(joinPath("/w/atlas", "./a/../b/c.md")).toBe("/w/atlas/b/c.md");
    expect(joinPath("/w", "../../../x.md")).toBe("/x.md");
  });
});
