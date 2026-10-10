import { describe, expect, it } from "vitest";
import { itemState, toolInfo } from "../test/fixtures";
import { clockTime } from "./format";
import { itemView, onJobNote, type ItemViewContext } from "./items";
import type { ItemState } from "./native";

const NOW = new Date(2026, 9, 4, 14, 0).getTime();
const MINUTE = 60_000;

/** An item whose state began six minutes before NOW. */
function item(overrides: Partial<ItemState> = {}): ItemState {
  return itemState({ startedAt: NOW - 60 * MINUTE, stateSince: NOW - 6 * MINUTE, ...overrides });
}

const ompTool = toolInfo({ path: "/usr/bin/omp" });

const context: ItemViewContext = { tool: ompTool, now: NOW, home: "/home/ana" };

describe("itemView: launch attempts", () => {
  it("names a starting launch after the tool and shows where it starts", () => {
    const view = itemView(item({ process: "starting" }), context);
    expect(view).toMatchObject({
      title: "Starting OMP…",
      badge: "start",
      pill: "starting",
      pillClass: "mute",
      sub: "~/w/atlas",
      stateText: "starting…",
      rowClass: "",
      subIsError: false,
    });
  });

  it("shows which conversation a starting resume continues", () => {
    const conversation = { tool: "omp", id: "c1", path: "/s/c1.jsonl" };
    expect(itemView(item({ process: "starting", conversation }), context).sub).toBe("resume “Fix the parser”");
  });

  it("falls back to the raw tool id when the tool is unknown", () => {
    const view = itemView(item({ process: "starting", tool: "aider" }), { ...context, tool: undefined });
    expect(view.title).toBe("Starting aider…");
  });

  it("reads a failed launch as a pending row with the failure as an error line", () => {
    const view = itemView(item({ process: "failed", failure: "omp: not found on PATH" }), context);
    expect(view).toMatchObject({
      title: "Couldn't start OMP",
      badge: "err",
      badgeGlyph: "×",
      pill: "",
      sub: "omp: not found on PATH",
      subIsError: true,
      stateText: "couldn't start",
      rowClass: "pending",
    });
  });

  it("explains a failed launch without a recorded reason", () => {
    expect(itemView(item({ process: "failed" }), context).sub).toBe("The launch failed before a process existed.");
  });
});

describe("itemView: exited rows", () => {
  const stateSince = NOW - 2 * MINUTE;

  it("reads an exited agent as stopped with an error, whatever the exit code", () => {
    const view = itemView(item({ process: "exited", exitCode: 0, stateSince }), context);
    expect(view).toMatchObject({
      title: "Fix the parser",
      badge: "err",
      badgeGlyph: "×",
      pill: "error",
      pillClass: "err",
      sub: `stopped with an error · ${clockTime(stateSince)}`,
      stateText: "stopped with an error",
      rowClass: "exited",
    });
  });

  it("shows a terminal's exit code in the pill and sub line", () => {
    const view = itemView(item({ kind: "terminal", process: "exited", exitCode: 1, stateSince }), context);
    expect(view).toMatchObject({
      badge: "exit",
      badgeGlyph: "■",
      pill: "exited 1",
      pillClass: "mute",
      sub: "exited · code 1",
      stateText: "exited 1",
      rowClass: "exited",
    });
  });

  it("shows when a terminal without an exit code ended", () => {
    const view = itemView(item({ kind: "terminal", process: "exited", exitCode: null, stateSince }), context);
    expect(view.pill).toBe("exited");
    expect(view.sub).toBe(`exited · ${clockTime(stateSince)}`);
  });
});

describe("itemView: running terminals", () => {
  const shell = { ...ompTool, id: "shell", label: "Shell", kind: "terminal" as const };
  const shellContext = { ...context, tool: shell };

  it("shows the tool and directory for a shell at its prompt", () => {
    const view = itemView(item({ kind: "terminal", tool: "shell", cwd: "/opt/x" }), shellContext);
    expect(view).toMatchObject({ badge: "", pill: "", sub: "Shell · /opt/x", stateText: "running", subModel: null });
  });

  it("marks a busy shell with the job's name and age", () => {
    const foregroundJob = { command: "cargo", sinceMs: NOW - 3 * MINUTE };
    const view = itemView(item({ kind: "terminal", tool: "shell", foregroundJob }), shellContext);
    expect(view).toMatchObject({
      badge: "job",
      pill: "cargo",
      pillClass: "job",
      sub: "Shell · ~/w/atlas · 3m",
      stateText: "running cargo",
    });
  });
});

describe("itemView: running agents", () => {
  it("joins tool, short model and state age in the sub line and exposes the model for its logo", () => {
    const view = itemView(item({ activity: "working", model: "anthropic/claude-opus-5-5" }), context);
    expect(view).toMatchObject({
      badge: "work",
      badgeGlyph: "",
      pill: "working",
      pillClass: "work",
      sub: "OMP · opus 5.5 · 6m",
      subModel: "anthropic/claude-opus-5-5",
      stateText: "working",
    });
  });

  it("leaves the model out when none is known", () => {
    expect(itemView(item({ activity: "working" }), context).sub).toBe("OMP · 6m");
  });

  it.each([
    ["approval", "needs you", "needs approval", "OMP · approval 6m"],
    ["question", "question", "has a question", "OMP · question 6m"],
    [null, "needs you", "needs you", "OMP · needs you 6m"],
  ] as const)("needs-you with %s attention: pill %s, state %s", (attention, pill, stateText, sub) => {
    const view = itemView(item({ activity: "needs-you", attention }), context);
    expect(view).toMatchObject({ badge: "need", badgeGlyph: "!", pill, pillClass: "need", stateText, sub });
  });

  it("shows a quieter pill for a your-turn the user has already seen", () => {
    const unread = itemView(item({ activity: "your-turn", unread: true }), context);
    const seen = itemView(item({ activity: "your-turn", unread: false }), context);
    expect(unread).toMatchObject({ badge: "", pill: "your turn", pillClass: "turn", stateText: "your turn" });
    expect(seen.pillClass).toBe("turn seen");
  });

  it("flags an agent-reported error", () => {
    const view = itemView(item({ activity: "error" }), context);
    expect(view).toMatchObject({ badge: "err", badgeGlyph: "×", pill: "error", pillClass: "err", stateText: "error" });
    expect(view.sub).toBe("OMP · 6m");
  });

  it("shows no badge or age for an agent whose status is unknown", () => {
    const view = itemView(item({ activity: "unknown", model: "gpt-6.1" }), context);
    expect(view).toMatchObject({ badge: "", pill: "", sub: "OMP · gpt 6.1 · running", stateText: "running" });
  });
});

describe("itemView: an agent on its own background jobs", () => {
  const test = { kind: "bash", label: "bun run test", startedAtMs: NOW - 5 * MINUTE };
  const dev = { kind: "bash", label: "vite dev", startedAtMs: NOW - 90 * MINUTE };
  const build = { kind: "task", label: "cargo build --release", startedAtMs: NOW - 80 * MINUTE };

  it("shows one job's label where the age sits and the age in the pill", () => {
    const view = itemView(
      item({
        activity: "on-job",
        model: "anthropic/claude-opus-5-5",
        stateSince: NOW - 4 * MINUTE,
        backgroundJobs: [test],
      }),
      context,
    );
    expect(view).toMatchObject({
      badge: "wait",
      pill: "on job 4m",
      pillClass: "wait",
      sub: "OMP · opus 5.5 · bun run test",
    });
  });

  it("counts several jobs in the pill and lists their labels oldest first", () => {
    const view = itemView(
      item({ activity: "on-job", stateSince: NOW - 75 * MINUTE, backgroundJobs: [dev, build] }),
      context,
    );
    expect(view.pill).toBe("on 2 jobs 1h");
    expect(view.sub).toBe("OMP · vite dev, cargo build --release");
  });

  it("names the background jobs in its accessible state", () => {
    expect(itemView(item({ activity: "on-job", backgroundJobs: [test] }), context).stateText).toBe(
      "waiting on 1 background job",
    );
    expect(itemView(item({ activity: "on-job", backgroundJobs: [dev, build] }), context).stateText).toBe(
      "waiting on 2 background jobs",
    );
  });

  it("puts each job's full label, kind and start time in the tooltip", () => {
    const view = itemView(item({ activity: "on-job", backgroundJobs: [dev, build] }), context);
    expect(view.tooltip).toBe(
      `vite dev — bash, since ${clockTime(dev.startedAtMs)}\ncargo build --release — task, since ${clockTime(build.startedAtMs)}`,
    );
  });

  it("tells the user in the context bar that they can type while the jobs run", () => {
    expect(onJobNote(item({ activity: "on-job", backgroundJobs: [dev, build] }))).toBe(
      "vite dev · cargo build --release · you can type; the agent resumes when they end",
    );
  });
});
