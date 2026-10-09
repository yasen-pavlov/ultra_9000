import { describe, expect, it } from "vitest";
import {
  basename,
  calendarDay,
  clockSeconds,
  clockTime,
  fullDate,
  highlight,
  matchesQuery,
  modelShort,
  projectInitials,
  recency,
  recencyBucket,
  shortAge,
  spokenAge,
  tildePath,
  trimTrailingSlashes,
} from "./format";

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Local wall-clock instant; months are 1-based here for readability. */
function local(year: number, month: number, day: number, hour = 12, minute = 0, second = 0): number {
  return new Date(year, month - 1, day, hour, minute, second).getTime();
}

describe("shortAge", () => {
  const now = local(2026, 7, 15, 12);

  it.each([
    [0, "<1m"],
    [59 * SECOND, "<1m"],
    [MINUTE, "1m"],
    [59 * MINUTE + 59 * SECOND, "59m"],
    [HOUR, "1h"],
    [23 * HOUR + 59 * MINUTE, "23h"],
    [DAY, "1d"],
    [9 * DAY + 5 * HOUR, "9d"],
  ])("%i ms elapsed reads %s", (elapsed, expected) => {
    expect(shortAge(now - elapsed, now)).toBe(expected);
  });

  it("clamps a future start (clock skew) to <1m", () => {
    expect(shortAge(now + 5 * MINUTE, now)).toBe("<1m");
  });
});

describe("spokenAge", () => {
  const now = local(2026, 7, 15, 12);

  it.each([
    [0, "1 second"],
    [1400, "1 second"],
    [1600, "2 seconds"],
    [40 * SECOND, "40 seconds"],
    [6 * MINUTE + 30 * SECOND, "6 min"],
    [3 * HOUR + 59 * MINUTE, "3 h"],
    [DAY + HOUR, "1 day"],
    [2 * DAY, "2 days"],
  ])("%i ms elapsed reads %s", (elapsed, expected) => {
    expect(spokenAge(now - elapsed, now)).toBe(expected);
  });

  it("never says zero seconds for a future instant", () => {
    expect(spokenAge(now + HOUR, now)).toBe("1 second");
  });
});

describe("recency", () => {
  const now = local(2026, 7, 15, 18);

  it.each([
    [local(2026, 7, 15, 17, 59, 30), "just now"],
    [local(2026, 7, 15, 17, 55), "5 min"],
    [local(2026, 7, 15, 0, 5), "17 h"],
    [local(2026, 7, 14, 23, 59), "yesterday"],
    [local(2026, 7, 9, 18), "6 d"],
    [local(2026, 7, 2, 18), "13 d"],
    [local(2026, 7, 1, 18), "2 wk"],
    [local(2026, 5, 14, 18), "8 wk"],
  ])("%i reads %s", (at, expected) => {
    expect(recency(at, now)).toBe(expected);
  });

  it("goes by calendar day, not 24-hour spans", () => {
    const justAfterMidnight = local(2026, 7, 15, 0, 10);
    expect(recency(local(2026, 7, 14, 23, 50), justAfterMidnight)).toBe("yesterday");
  });

  it("falls back to the calendar day from nine weeks on", () => {
    const at = local(2026, 5, 13, 18);
    expect(recency(at, now)).toBe(calendarDay(at, now));
    expect(recency(at, now)).toContain("13");
  });

  it("treats future instants as just now", () => {
    expect(recency(now + 10 * MINUTE, now)).toBe("just now");
  });
});

describe("recencyBucket", () => {
  const now = local(2026, 7, 20, 12);

  it.each([
    [local(2026, 7, 20, 0, 1), "Today"],
    [local(2026, 7, 21, 9), "Today"],
    [local(2026, 7, 19, 23), "Yesterday"],
    [local(2026, 7, 14), "This week"],
    [local(2026, 7, 13), "Earlier this month"],
    [local(2026, 7, 1), "Earlier this month"],
  ])("%i is %s", (at, expected) => {
    expect(recencyBucket(at, now)).toBe(expected);
  });

  it("names earlier months of this year without the year", () => {
    const bucket = recencyBucket(local(2026, 6, 30), now);
    expect(bucket).not.toMatch(/\d/);
    expect(bucket).not.toBe(recencyBucket(local(2026, 5, 30), now));
  });

  it("adds the year for months of other years", () => {
    expect(recencyBucket(local(2025, 7, 20), now)).toMatch(/ 2025$/);
  });

  it("a week-old item from the previous month is not 'earlier this month'", () => {
    const early = local(2026, 8, 3, 12);
    expect(recencyBucket(local(2026, 7, 27), early)).not.toBe("Earlier this month");
    expect(recencyBucket(local(2026, 7, 27), early)).not.toMatch(/\d/);
  });
});

describe("calendarDay", () => {
  it("omits the year within the current year and shows it otherwise", () => {
    const now = local(2026, 9, 30);
    expect(calendarDay(local(2026, 1, 5), now)).toMatch(/\b5\b/);
    expect(calendarDay(local(2026, 1, 5), now)).not.toContain("2026");
    expect(calendarDay(local(2025, 1, 5), now)).toContain("2025");
  });
});

describe("clock formats", () => {
  const at = local(2026, 9, 9, 15, 4, 9);

  it("reads 24-hour time whatever the locale's hour cycle", () => {
    expect(clockTime(at)).toBe("15:04");
    expect(clockTime(local(2026, 9, 9, 0, 7))).toBe("00:07");
  });

  it("adds seconds for the precise variant", () => {
    expect(clockSeconds(at)).toBe("15:04:09");
  });

  it("spells the full date with the 24-hour time", () => {
    const full = fullDate(at);
    expect(full).toContain("15:04");
    expect(full).toMatch(/\b9\b/);
  });
});

describe("modelShort", () => {
  it.each([
    ["anthropic/claude-opus-5-5", "opus 5.5"],
    ["claude-sonnet-4-6", "sonnet 4.6"],
    ["openai-codex/gpt-6.1-sol", "gpt 6.1 sol"],
    ["openrouter/moonshotai/kimi-k3", "kimi k3"],
    ["claude-opus-4-1-20250805", "opus 4.1 20250805"],
    ["claude-x-1-2-3", "x 1.2 3"],
    ["model-10-20", "model 10.20"],
    ["a-1-2b", "a 1 2b"],
    ["no-dashes-here", "no dashes here"],
    ["gemini-3-pro", "gemini 3 pro"],
  ])("%s → %s", (model, expected) => {
    expect(modelShort(model)).toBe(expected);
  });
});

describe("tildePath", () => {
  it("abbreviates the home directory and paths below it", () => {
    expect(tildePath("/home/ana", "/home/ana")).toBe("~");
    expect(tildePath("/home/ana/w/atlas", "/home/ana")).toBe("~/w/atlas");
  });

  it("leaves sibling paths sharing the home prefix alone", () => {
    expect(tildePath("/home/anabel/w", "/home/ana")).toBe("/home/anabel/w");
  });

  it("never abbreviates with an empty or root home", () => {
    expect(tildePath("/w/atlas", "")).toBe("/w/atlas");
    expect(tildePath("/w/atlas", "/")).toBe("/w/atlas");
  });
});

describe("basename", () => {
  it.each([
    ["/w/atlas", "atlas"],
    ["/w/atlas///", "atlas"],
    ["atlas", "atlas"],
    ["/", "/"],
    ["", ""],
  ])("%s → %s", (path, expected) => {
    expect(basename(path)).toBe(expected);
  });
});

describe("trimTrailingSlashes", () => {
  it.each([
    ["", ""],
    ["/", ""],
    ["//", ""],
    ["a", "a"],
    ["a/", "a"],
    ["a///", "a"],
    ["/w/atlas", "/w/atlas"],
    ["/w/atlas///", "/w/atlas"],
    ["/w//atlas/", "/w//atlas"],
    ["a/b/c//", "a/b/c"],
    ["/ /", "/ "],
  ])("%j → %j", (path, expected) => {
    expect(trimTrailingSlashes(path)).toBe(expected);
  });

  it("keeps inner slashes and drops only the trailing run", () => {
    expect(trimTrailingSlashes("/w//atlas///")).toBe("/w//atlas");
    expect(trimTrailingSlashes("////")).toBe("");
  });

  it("stays linear on long slash runs that do not end the string", () => {
    const pathological = `${"/".repeat(200_000)}x`;
    const started = performance.now();
    expect(trimTrailingSlashes(pathological)).toBe(pathological);
    expect(basename(pathological)).toBe("x");
    expect(basename(`${"/".repeat(200_000)}x${"/".repeat(200_000)}`)).toBe("x");
    expect(performance.now() - started).toBeLessThan(250);
  });
});

describe("projectInitials", () => {
  it("takes the first two letters or digits, skipping punctuation", () => {
    expect(projectInitials("atlas")).toBe("at");
    expect(projectInitials("_my-app")).toBe("my");
    expect(projectInitials("über")).toBe("üb");
    expect(projectInitials("9k")).toBe("9k");
  });

  it("falls back to the raw name when it has no letters", () => {
    expect(projectInitials("--")).toBe("--");
  });
});

describe("highlight", () => {
  it("splits around every case-insensitive occurrence, keeping original casing", () => {
    expect(highlight("Fix fixture FIX", "fix")).toEqual([
      { text: "Fix", match: true },
      { text: " ", match: false },
      { text: "fix", match: true },
      { text: "ture ", match: false },
      { text: "FIX", match: true },
    ]);
  });

  it("trims the query and returns the whole text unmarked for a blank one", () => {
    expect(highlight("atlas", "   ")).toEqual([{ text: "atlas", match: false }]);
    expect(highlight("atlas", " las ")).toEqual([
      { text: "at", match: false },
      { text: "las", match: true },
    ]);
  });

  it("returns one unmarked part when nothing matches", () => {
    expect(highlight("atlas", "zz")).toEqual([{ text: "atlas", match: false }]);
  });
});

describe("matchesQuery", () => {
  it("matches any field case-insensitively and ignores missing fields", () => {
    expect(matchesQuery("ATL", null, undefined, "the atlas project")).toBe(true);
    expect(matchesQuery("zzz", "atlas", null)).toBe(false);
  });

  it("matches everything for a blank query", () => {
    expect(matchesQuery("  ")).toBe(true);
  });
});
