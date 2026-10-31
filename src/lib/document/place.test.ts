import { describe, expect, it } from "vitest";
import { capturePlace, currentHeadingIndex, headingStep, restorePlace } from "./place";

const before = [
  { id: "user-content-summary", top: 0 },
  { id: "user-content-root-cause", top: 800 },
  { id: "user-content-the-fix", top: 1600 },
];

describe("place keeping across a re-render", () => {
  it("records the nearest heading at or above the view's top and the offset into it", () => {
    expect(capturePlace(before, 950)).toEqual({ id: "user-content-root-cause", offset: 150 });
    expect(capturePlace(before, 800)).toEqual({ id: "user-content-root-cause", offset: 0 });
  });

  it("keeps the reader on the same words when a replace moves the heading", () => {
    const place = capturePlace(before, 950);
    // The file was rewritten: 300 px of new text arrived above “Root cause”.
    const after = [
      { id: "user-content-summary", top: 0 },
      { id: "user-content-root-cause", top: 1100 },
      { id: "user-content-the-fix", top: 1900 },
    ];
    expect(restorePlace(place, after)).toEqual({ scrollTop: 1250, kept: true });
  });

  it("goes back to the top, and says so, when the place's heading did not survive", () => {
    const place = capturePlace(before, 950);
    expect(restorePlace(place, [{ id: "user-content-summary", top: 0 }])).toEqual({ scrollTop: 0, kept: false });
  });

  it("keeps a place above the first heading as a plain offset", () => {
    const headings = [{ id: "user-content-summary", top: 400 }];
    const place = capturePlace(headings, 120);
    expect(place).toEqual({ id: null, offset: 120 });
    expect(restorePlace(place, [{ id: "user-content-summary", top: 900 }])).toEqual({ scrollTop: 120, kept: true });
  });
});

describe("headings and the reader's position", () => {
  const headings = [
    { id: "a", top: 0 },
    { id: "b", top: 400 },
    { id: "c", top: 900 },
    { id: "d", top: 1500 },
  ];

  it("counts the heading a jump just landed on (8 px below the top) as the current one", () => {
    expect(currentHeadingIndex(headings, 392)).toBe(1);
    expect(currentHeadingIndex(headings, 300)).toBe(0);
  });

  it("moves one heading per press, even while the previous jump is still scrolling", () => {
    const first = headingStep(headings, 0, null, 1);
    expect(first).toBe(1);
    const second = headingStep(headings, 10, first, 1);
    const third = headingStep(headings, 30, second, 1);
    expect([second, third]).toEqual([2, 3]);
    expect(headingStep(headings, 30, third, 1)).toBeNull();
  });

  it("goes back to the current section's heading first, then to the one before", () => {
    expect(headingStep(headings, 600, null, -1)).toBe(1);
    expect(headingStep(headings, 392, null, -1)).toBe(0);
  });
});
