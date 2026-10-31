import { describe, expect, it } from "vitest";
import { acceptsText, wantsText } from "./surface-state";

describe("document text fetching", () => {
  it("fetches when the visible surface sees a new revision", () => {
    expect(wantsText({ status: "ok", revision: 3 }, 2, true)).toBe(true);
    expect(wantsText({ status: "ok", revision: 3 }, 3, true)).toBe(false);
  });

  it("does not fetch while hidden (another item or a sheet is shown)", () => {
    expect(wantsText({ status: "ok", revision: 3 }, 2, false)).toBe(false);
  });

  it("keeps a snapshot that lags behind a newer reply from fetching again", () => {
    expect(wantsText({ status: "ok", revision: 4 }, 5, true)).toBe(false);
  });

  it("fetches the last good text of a missing or unreadable file, never of a too-large one", () => {
    expect(wantsText({ status: "missing", revision: 2 }, -1, true)).toBe(true);
    expect(wantsText({ status: "unreadable", revision: 0 }, -1, true)).toBe(false);
    expect(wantsText({ status: "too-large", revision: 2 }, -1, true)).toBe(false);
  });

  it("never lets an older reply replace a newer one already shown", () => {
    expect(acceptsText(5, 4)).toBe(true);
    expect(acceptsText(4, 5)).toBe(false);
    expect(acceptsText(5, 5)).toBe(false);
  });

  it("fetches again after an explicit reload, even when the revision did not change", () => {
    expect(wantsText({ status: "ok", revision: 1 }, 1, true, true)).toBe(true);
    expect(wantsText({ status: "ok", revision: 1 }, 1, false, true)).toBe(false);
    expect(wantsText({ status: "too-large", revision: 1 }, 1, true, true)).toBe(false);
  });
});
