import { describe, expect, it } from "vitest";
import { fittingHints } from "./hints";

describe("footer hints that fit", () => {
  it("keeps every hint when the row fits, counting gaps only between hints", () => {
    expect(fittingHints([40, 40, 40], 12, 144)).toBe(3);
    expect(fittingHints([40], 12, 40)).toBe(1);
  });

  it("drops trailing hints that would overflow, never a middle one", () => {
    expect(fittingHints([40, 40, 40], 12, 143)).toBe(2);
    expect(fittingHints([48, 62, 61, 58, 61], 12, 294)).toBe(4);
  });

  it("shows nothing rather than a clipped first hint", () => {
    expect(fittingHints([80, 20], 12, 60)).toBe(0);
    expect(fittingHints([], 12, 300)).toBe(0);
  });
});
