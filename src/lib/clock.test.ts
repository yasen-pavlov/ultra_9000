import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clock } from "./clock.svelte";

describe("clock", () => {
  beforeEach(() => {
    vi.useFakeTimers({ now: new Date(2026, 9, 4, 12, 0, 0) });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("catches up to the wall clock on start, then ticks every 15 seconds until stopped", () => {
    vi.setSystemTime(new Date(2026, 9, 4, 12, 30, 0));
    const stop = clock.start();
    const started = Date.now();
    expect(clock.now).toBe(started);

    vi.advanceTimersByTime(14_999);
    expect(clock.now).toBe(started);
    vi.advanceTimersByTime(1);
    expect(clock.now).toBe(started + 15_000);

    stop();
    vi.advanceTimersByTime(60_000);
    expect(clock.now).toBe(started + 15_000);
  });
});
