// How one item reads everywhere (sidebar row, collapsed icon, palette, context bar, quit list):
// state glyph + word, never colour alone (design board 20/21).
import { clockTime, modelShort, shortAge, tildePath } from "./format";
import type { Attention, ItemState, ToolInfo } from "./native";

export type Badge = "" | "start" | "work" | "wait" | "need" | "err" | "exit" | "job";

export interface ItemView {
  /** Row title; launch attempts read "Starting OMP…" / "Couldn't start Codex". */
  title: string;
  badge: Badge;
  badgeGlyph: string;
  pill: string;
  pillClass: string;
  /** Secondary line (comfortable density). */
  sub: string;
  /** The model whose short name appears in `sub`, so callers can put its provider logo before it. */
  subModel: string | null;
  /** Row tooltip: the sub line, or for an agent on job its jobs' full labels, kinds and start times. */
  tooltip: string;
  subIsError: boolean;
  /** Short state wording: "working", "needs approval", "your turn", "exited 1". */
  stateText: string;
  /** Row modifier: exited/error rows dim their tile; failed attempts read as pending. */
  rowClass: "" | "exited" | "pending";
}

export interface ItemViewContext {
  tool: ToolInfo | undefined;
  now: number;
  home: string;
}

/** What a needs-you item is asking for. */
const NEEDS_YOU_TEXT: Readonly<Record<NonNullable<Attention>, string>> = {
  approval: "needs approval",
  question: "has a question",
};

/** A running item's reported activity, in words. */
function runningStateText(item: ItemState): string {
  switch (item.activity) {
    case "working":
      return "working";
    case "on-job":
      return `waiting on ${jobCount(item.backgroundJobs.length)}`;
    case "needs-you":
      return item.attention === null ? "needs you" : NEEDS_YOU_TEXT[item.attention];
    case "your-turn":
      return "your turn";
    case "error":
      return "error";
    case "unknown":
      return item.foregroundJob === null ? "running" : `running ${item.foregroundJob.command}`;
  }
}

function stateText(item: ItemState): string {
  switch (item.process) {
    case "starting":
      return "starting…";
    case "failed":
      return "couldn't start";
    case "exited":
      if (item.kind === "agent") return "stopped with an error";
      return item.exitCode === null ? "exited" : `exited ${item.exitCode}`;
    case "running":
      return runningStateText(item);
  }
}

/** Sub-line tail (board 21, 2026-10-04): the age; the pill carries the state. Needs-you keeps its request kind. */
function runningAgentStatus(item: ItemState, now: number): string {
  const age = shortAge(item.stateSince, now);
  switch (item.activity) {
    case "needs-you":
      return `${item.attention ?? "needs you"} ${age}`;
    case "on-job":
      // Board 28: job labels sit where the age normally is, oldest first; the pill carries the age.
      return item.backgroundJobs.map((job) => job.label).join(", ");
    case "working":
    case "your-turn":
    case "error":
      return age;
    case "unknown":
      return "running";
  }
}

function applyExited(view: ItemView, item: ItemState): void {
  view.rowClass = "exited";
  if (item.kind === "agent") {
    view.badge = "err";
    view.badgeGlyph = "×";
    view.pill = "error";
    view.pillClass = "err";
    view.sub = `stopped with an error · ${clockTime(item.stateSince)}`;
  } else {
    view.badge = "exit";
    view.badgeGlyph = "■";
    view.pill = view.stateText;
    view.pillClass = "mute";
    view.sub = item.exitCode === null ? `exited · ${clockTime(item.stateSince)}` : `exited · code ${item.exitCode}`;
  }
}

function applyRunningTerminal(view: ItemView, item: ItemState, label: string, context: ItemViewContext): void {
  view.sub = `${label} · ${tildePath(item.cwd, context.home)}`;
  // A busy shell (board 21, decision 20): ▸ badge and the job's name, calm and static.
  if (item.foregroundJob !== null) {
    view.sub += ` · ${shortAge(item.foregroundJob.sinceMs, context.now)}`;
    view.badge = "job";
    view.pill = item.foregroundJob.command;
    view.pillClass = "job";
  }
}

function applyRunningAgent(view: ItemView, item: ItemState, label: string, now: number): void {
  view.sub = [label, item.model === null ? null : modelShort(item.model), runningAgentStatus(item, now)]
    .filter((part) => part !== null)
    .join(" · ");
  view.subModel = item.model;
  switch (item.activity) {
    case "working":
      view.badge = "work";
      view.pill = "working";
      view.pillClass = "work";
      break;
    case "on-job": {
      // Board 28 (decision 22): the agent yielded while its own background jobs run.
      const count = item.backgroundJobs.length;
      const jobs = count === 1 ? "on job" : `on ${count} jobs`;
      view.badge = "wait";
      view.pill = `${jobs} ${shortAge(item.stateSince, now)}`;
      view.pillClass = "wait";
      view.tooltip = item.backgroundJobs
        .map((job) => `${job.label} — ${job.kind}, since ${clockTime(job.startedAtMs)}`)
        .join("\n");
      break;
    }
    case "needs-you":
      view.badge = "need";
      view.badgeGlyph = "!";
      view.pill = item.attention === "question" ? "question" : "needs you";
      view.pillClass = "need";
      break;
    case "your-turn":
      // Every your-turn shows a pill; a seen one is quieter (board 21, 2026-10-04).
      view.pill = "your turn";
      view.pillClass = item.unread ? "turn" : "turn seen";
      break;
    case "error":
      view.badge = "err";
      view.badgeGlyph = "×";
      view.pill = "error";
      view.pillClass = "err";
      break;
    case "unknown":
      break;
  }
}

function jobCount(count: number): string {
  return count === 1 ? "1 background job" : `${count} background jobs`;
}

/** Context bar line for an agent on job (board 28): every label, then what the user may do meanwhile. */
export function onJobNote(item: ItemState): string {
  return [...item.backgroundJobs.map((job) => job.label), "you can type; the agent resumes when they end"].join(" · ");
}

export function itemView(item: ItemState, context: ItemViewContext): ItemView {
  const label = context.tool?.label ?? item.tool;
  const view: ItemView = {
    title: item.title,
    badge: "",
    badgeGlyph: "",
    pill: "",
    pillClass: "",
    sub: "",
    subModel: null,
    tooltip: "",
    subIsError: false,
    stateText: stateText(item),
    rowClass: "",
  };

  switch (item.process) {
    case "starting":
      view.title = `Starting ${label}…`;
      view.badge = "start";
      view.pill = "starting";
      view.pillClass = "mute";
      view.sub = item.conversation !== null ? `resume “${item.title}”` : tildePath(item.cwd, context.home);
      break;
    case "failed":
      view.title = `Couldn't start ${label}`;
      view.badge = "err";
      view.badgeGlyph = "×";
      view.sub = item.failure ?? "The launch failed before a process existed.";
      view.subIsError = true;
      view.rowClass = "pending";
      break;
    case "exited":
      applyExited(view, item);
      break;
    case "running":
      if (item.kind === "terminal") applyRunningTerminal(view, item, label, context);
      else applyRunningAgent(view, item, label, context.now);
      break;
  }
  if (view.tooltip === "") view.tooltip = view.sub;
  return view;
}
