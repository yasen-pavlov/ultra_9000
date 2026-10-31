import { describe, expect, it } from "vitest";
import { followAfterAction, followAfterScroll, type ScrollFacts } from "./follow";

function scroll(overrides: Partial<ScrollFacts>): ScrollFacts {
  return { follow: false, movedUp: false, atEnd: false, live: false, programmatic: false, ...overrides };
}

describe("follow on scroll", () => {
  it("turns off at once on any upward move", () => {
    expect(followAfterScroll(scroll({ follow: true, movedUp: true, atEnd: true, live: true }))).toBe(false);
  });

  it("turns on when the reader reaches the end while the file is live", () => {
    expect(followAfterScroll(scroll({ atEnd: true, live: true }))).toBe(true);
    expect(followAfterScroll(scroll({ atEnd: true, live: false }))).toBe(false);
  });

  it("ignores the reader's own scrolling (place restore, following the end)", () => {
    expect(followAfterScroll(scroll({ follow: true, movedUp: true, programmatic: true }))).toBe(true);
    expect(followAfterScroll(scroll({ atEnd: true, live: true, programmatic: true }))).toBe(false);
  });
});

describe("follow on reading keys", () => {
  it("f toggles", () => {
    expect(followAfterAction(false, { kind: "follow" }, false)).toBe(true);
    expect(followAfterAction(true, { kind: "follow" }, true)).toBe(false);
  });

  it("k, ↑, u, PageUp, previous heading and gg turn it off; moving down keeps it", () => {
    expect(followAfterAction(true, { kind: "scroll", unit: "lines", direction: -1 }, true)).toBe(false);
    expect(followAfterAction(true, { kind: "scroll", unit: "page", direction: -1 }, true)).toBe(false);
    expect(followAfterAction(true, { kind: "heading", direction: -1 }, true)).toBe(false);
    expect(followAfterAction(true, { kind: "top" }, true)).toBe(false);
    expect(followAfterAction(true, { kind: "scroll", unit: "lines", direction: 1 }, true)).toBe(true);
  });

  it("G re-engages only while the file is live", () => {
    expect(followAfterAction(false, { kind: "end" }, true)).toBe(true);
    expect(followAfterAction(false, { kind: "end" }, false)).toBe(false);
  });

  it("other actions leave it alone", () => {
    expect(followAfterAction(true, { kind: "copy-path" }, false)).toBe(true);
    expect(followAfterAction(false, { kind: "find" }, true)).toBe(false);
  });
});
