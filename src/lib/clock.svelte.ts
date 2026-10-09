// A coarse wall clock for relative durations ("6m", "3h", "yesterday"). It only re-renders text;
// it never issues native commands, and native snapshots are not refetched on its ticks.
const TICK_MS = 15_000;

class Clock {
  now = $state(Date.now());

  /** Starts ticking; returns the stop function (for an $effect cleanup). */
  start(): () => void {
    this.now = Date.now();
    const timer = setInterval(() => {
      this.now = Date.now();
    }, TICK_MS);
    return () => clearInterval(timer);
  }
}

export const clock = new Clock();
