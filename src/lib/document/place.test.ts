import { describe, expect, it } from "vitest";
import { capturePlace, restorePlace } from "./place";

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
