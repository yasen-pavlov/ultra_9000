import { describe, expect, it } from "vitest";
import { ago, statusLine, type StatusInput } from "./status";

const NOW = 1_000_000;

function input(overrides: Partial<StatusInput> = {}): StatusInput {
  return {
    now: NOW,
    lastEvent: "opened",
    lastEventAtMs: 0,
    follow: false,
    linesBelow: 0,
    replaced: null,
    copiedAt: null,
    ...overrides,
  };
}

describe("the bottom-right line", () => {
  it("says nothing about a file at rest", () => {
    expect(statusLine(input())).toBeNull();
  });

  it("reports a write in whole seconds and fades a minute after it", () => {
    expect(statusLine(input({ lastEvent: "updated", lastEventAtMs: NOW - 2_400 }))).toEqual({
      text: "updated 2 s ago",
      visible: true,
      clickable: false,
    });
    expect(statusLine(input({ lastEvent: "updated", lastEventAtMs: NOW - 61_000 }))?.visible).toBe(false);
  });

  it("stays while following, and offers the click", () => {
    expect(statusLine(input({ follow: true, lastEvent: "updated", lastEventAtMs: NOW - 1_000 }))).toEqual({
      text: "updated 1 s ago · following",
      visible: true,
      clickable: true,
    });
    expect(statusLine(input({ follow: true, lastEvent: "updated", lastEventAtMs: NOW - 120_000 }))?.text).toBe(
      "updated 2 min ago · following",
    );
  });

  it("counts what arrived below while follow is off", () => {
    expect(statusLine(input({ linesBelow: 14, lastEvent: "updated", lastEventAtMs: NOW - 1_000 }))?.text).toBe(
      "+14 lines below · f to follow",
    );
    expect(statusLine(input({ linesBelow: 1 }))?.text).toBe("+1 line below · f to follow");
  });

  it("says whether a replace kept the reader's place, for ten seconds", () => {
    const kept = input({
      lastEvent: "replaced",
      lastEventAtMs: NOW - 3_000,
      replaced: { at: NOW - 3_000, kept: true },
    });
    expect(statusLine(kept)?.text).toBe("replaced 3 s ago · kept your place");
    const lost = { ...kept, replaced: { at: NOW - 3_000, kept: false } };
    expect(statusLine(lost)?.text).toBe("replaced 3 s ago · back to top");
    expect(statusLine({ ...kept, now: NOW + 8_000 })?.text).toBe("updated 11 s ago");
  });

  it("shows “copied” briefly over everything else", () => {
    expect(statusLine(input({ follow: true, copiedAt: NOW - 500 }))?.text).toBe("copied");
    expect(statusLine(input({ follow: true, copiedAt: NOW - 2_000 }))?.text).toBe("following");
  });

  it("formats ages as the board writes them", () => {
    expect(ago(NOW - 200, NOW)).toBe("1 s ago");
    expect(ago(NOW - 59_000, NOW)).toBe("59 s ago");
    expect(ago(NOW - 180_000, NOW)).toBe("3 min ago");
  });
});
