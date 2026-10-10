/**
 * Ultra 9000 status link for OMP.
 *
 * Ultra 9000 launches OMP with `-e <this file>`, `ULTRA9000_OMP_LINK=<unix socket path>` and
 * `ULTRA9000_OMP_TOKEN_FILE=<file holding the token>`. This extension reports the main agent's
 * session, model, name and turn status to that socket as NDJSON, one JSON object per line, client
 * to server only. It only observes OMP: it registers no tools, commands or shortcuts and never
 * writes to the session. Without a usable socket path and token it does nothing.
 *
 * OMP evaluates this module once per process and rebinds the default-export factory for every
 * subagent, advisor and `/tan` clone it runs in-process, so the link and the state machine live at
 * module level and each handler filters on `ctx.agent.kind`.
 */
import { readFileSync, unlinkSync } from "node:fs";
import { createConnection, type Socket } from "node:net";

/**
 * Reads the link once at load. The token arrives in a one-shot file that is removed right away:
 * JS cannot unset variables in the native environment that OMP's tool subprocesses inherit, so
 * they only ever see the socket path and the name of a file that no longer exists.
 */
function readLink(): { path: string; token: string } | undefined {
  const path = process.env.ULTRA9000_OMP_LINK;
  const tokenFile = process.env.ULTRA9000_OMP_TOKEN_FILE;
  delete process.env.ULTRA9000_OMP_LINK;
  delete process.env.ULTRA9000_OMP_TOKEN_FILE;
  if (!tokenFile) return undefined;
  let token = "";
  try {
    token = readFileSync(tokenFile, "utf8").trimEnd();
    unlinkSync(tokenFile);
  } catch {
    // A missing or unreadable token file leaves the extension inert.
  }
  return path && /^[0-9a-f]{32}$/.test(token) ? { path, token } : undefined;
}
const LINK = readLink();

const POLL_MS = 500;
/** Quiet polls (10 s) after which a run whose start/end counts never balance settles as idle. */
const FALLBACK_POLLS = 20;
const RECONNECT_MS = 2_000;
const MAX_JOBS = 8;
const MAX_LINE_BYTES = 64 * 1024;
/** Message kinds in send order: the receiver binds the session before it reads anything about it. */
const KINDS = ["session", "model", "name", "state"] as const;
type Kind = (typeof KINDS)[number];

// The slice of OMP's extension API used here (OMP 18.8.7, `extensibility/extensions/types.ts`).
interface AgentMessage {
  role?: string;
  stopReason?: string;
  timestamp?: number;
}
interface AsyncJob {
  type: string;
  label: string;
  startTime: number;
}
interface Context {
  agent?: { kind: "main" | "sub" };
  model: { provider: string; id: string } | undefined;
  sessionManager: { getSessionId(): string; getSessionFile(): string | undefined; getCwd(): string };
  isIdle(): boolean;
  hasPendingMessages(): boolean;
  getAsyncJobSnapshot(): { running: AsyncJob[]; delivery: { queued: number; delivering: boolean } } | null;
  setInterval(callback: () => void, ms: number): unknown;
}
interface Event {
  toolCallId?: string;
  toolName?: string;
  messages?: AgentMessage[];
  willContinue?: boolean;
}
interface ExtensionApi {
  on(event: string, handler: (event: Event, ctx: Context) => void): void;
  getSessionName(): string | undefined;
  /** OMP's own exports, injected into every extension. */
  pi?: { VERSION?: unknown };
}

type Job = { label: string; jobType: string; startedAt: number };
type Status = { state: "working" | "done" | "idle" | "error" } | { state: "waiting"; jobs: Job[] };

// Turn state of the main agent plus the prompts any agent in the process is waiting on.
let status: Status = { state: "idle" };
const approvals = new Set<string>();
const questions = new Set<string>();
let retrying = false;
let compacting = false;
/**
 * Main runs started and ended in this session. OMP emits `agent_end` to extensions without awaiting
 * it and only after other extensions' `session_stop` handlers, so slow handlers can deliver it late
 * or out of order: a run is over only once every started run has ended.
 */
let starts = 0;
let ends = 0;
/**
 * What a settle reports: of the ends received since the last settle, the one with the newest last
 * message, if that message is newer than `watermark` (the newest message already reported, or the
 * session change). Otherwise the run produced nothing new and reports `idle`. Message times, not
 * arrival order or handler delivery times, decide the outcome.
 */
let candidate: { messages: AgentMessage[]; at: number } | undefined;
let watermark = 0;
let quietPolls = 0;
/**
 * Whether the newest main `agent_end` said OMP continues. Only then is queued input proof of a
 * coming turn: after a terminal end OMP may keep its queue blocked (failed drain) indefinitely.
 */
let lastEndContinues = false;

// The link: latest line per kind, and what this connection has already been sent.
let ompVersion = "unknown";
let socket: Socket | undefined;
let lastConnectAt = -Infinity;
let closed = false;
let poll: unknown;
const latest = new Map<Kind, string>();
const sent = new Map<Kind, string>();

/**
 * Reports the run as over, from the candidate end the way OMP's TUI reads `agent_end` messages, or
 * as `idle` for the fallback, which leaves the candidate for a later barrier settle.
 */
function settle(withOutcome: boolean): void {
  approvals.clear();
  questions.clear();
  quietPolls = 0;
  let messages: AgentMessage[] | undefined;
  if (withOutcome) {
    if (candidate && candidate.at > watermark) {
      messages = candidate.messages;
      watermark = candidate.at;
    }
    candidate = undefined;
  }
  const last = messages?.findLast((message) => message.role === "assistant");
  if (retrying || !last || last.stopReason === "aborted") status = { state: "idle" };
  else if (last.stopReason === "error") status = { state: "error" };
  else status = { state: "done" };
}

/**
 * Settles the main run once every started run has ended and OMP is quiet: not streaming, nothing
 * queued, no retry or compaction in flight and no background job that could wake it, mirroring
 * OMP's own settle watchers (`event-controller.ts#finishWhenRunSettles`, `rpc-session-settle.ts`).
 * Background jobs show as `waiting`. A newer end arriving after a settle re-settles only through
 * the same barrier. If the counts never balance (a duplicated `agent_start`, a lost end), a long
 * quiet stretch settles as `idle` so the status cannot stay `working`.
 */
function checkSettle(ctx: Context, polling: boolean): void {
  const running = status.state === "working" || status.state === "waiting";
  if (!running && !(candidate && candidate.at > watermark)) return;
  if (!ctx.isIdle() || (lastEndContinues && ctx.hasPendingMessages()) || retrying || compacting) {
    quietPolls = 0;
    return;
  }
  const jobs = ctx.getAsyncJobSnapshot();
  if (jobs && (jobs.running.length > 0 || jobs.delivery.queued > 0 || jobs.delivery.delivering)) {
    quietPolls = 0;
    if (!running || ends < starts) return;
    status = {
      state: "waiting",
      jobs: jobs.running
        .slice(0, MAX_JOBS)
        .map((job) => ({ label: job.label.toWellFormed(), jobType: job.type, startedAt: job.startTime })),
    };
    return;
  }
  if (ends >= starts) settle(true);
  else if (running && polling && ++quietPolls >= FALLBACK_POLLS) settle(false);
}

function stateMessage(): Record<string, unknown> {
  if (approvals.size > 0) return { type: "state", state: "needs-you", kind: "approval" };
  if (questions.size > 0) return { type: "state", state: "needs-you", kind: "question" };
  return { type: "state", ...status };
}

function update(kind: Kind, message: Record<string, unknown>): void {
  const line = JSON.stringify(message);
  if (latest.get(kind) === line || Buffer.byteLength(line) >= MAX_LINE_BYTES) return;
  latest.set(kind, line);
  // A new session is a new record on the receiver: repeat everything known about it.
  if (kind === "session") for (const other of KINDS) if (other !== "session") sent.delete(other);
}

/** Sends every line that changed since this connection last saw it, connecting first if needed. */
function flush(): void {
  if (closed || !LINK) return;
  if (!socket) {
    if (Date.now() - lastConnectAt < RECONNECT_MS) return;
    socket = connect(LINK);
  }
  for (const kind of KINDS) {
    const line = latest.get(kind);
    if (line === undefined || sent.get(kind) === line) continue;
    socket.write(`${line}\n`);
    sent.set(kind, line);
  }
}

function connect(link: { path: string; token: string }): Socket {
  lastConnectAt = Date.now();
  sent.clear();
  const connection = createConnection(link.path);
  connection.unref();
  const drop = (): void => {
    connection.destroy();
    if (socket === connection) socket = undefined;
  };
  connection.on("error", drop);
  connection.on("close", drop);
  connection.write(`${JSON.stringify({ v: 1, type: "hello", token: link.token, omp: ompVersion })}\n`);
  return connection;
}

function guard(run: () => void): void {
  try {
    run();
  } catch {
    // Status reporting must never disturb OMP; the next event or poll reports again.
  }
}

export default function ultra9000Status(pi: ExtensionApi): void {
  if (!LINK) return;
  if (typeof pi.pi?.VERSION === "string") ompVersion = pi.pi.VERSION;

  /**
   * Re-reads the main session's identity, model and name and sends whatever changed. OMP has no
   * event for a model switch or a late auto-title, so a managed poll re-checks them, and pending
   * settles, while the session lives.
   */
  const report = (ctx: Context): void => {
    poll ??= ctx.setInterval(
      () =>
        guard(() => {
          checkSettle(ctx, true);
          report(ctx);
        }),
      POLL_MS,
    );
    const session = ctx.sessionManager;
    const file = session.getSessionFile();
    const model = ctx.model;
    const name = pi.getSessionName();
    update("session", {
      type: "session",
      id: session.getSessionId(),
      file: file === undefined ? null : file.toWellFormed(),
      cwd: session.getCwd().toWellFormed(),
    });
    update("model", { type: "model", model: model ? `${model.provider}/${model.id}`.toWellFormed() : null });
    update("name", { type: "name", name: name === undefined ? null : name.toWellFormed() });
    update("state", stateMessage());
    flush();
  };

  /**
   * Turn lifecycle and session identity come from the main agent only; an approval or question
   * from any agent needs the user. A missing `ctx.agent` is the main agent.
   */
  const listen = (scope: "main" | "any", event: string, handler: (event: Event, ctx: Context) => void): void =>
    pi.on(event, (payload, ctx) =>
      guard(() => {
        const main = (ctx.agent?.kind ?? "main") === "main";
        if (!main && scope === "main") return;
        handler(payload, ctx);
        if (main) {
          report(ctx);
        } else {
          update("state", stateMessage());
          flush();
        }
      }),
    );

  const startSession = (_event: Event, ctx: Context): void => {
    starts = 0;
    ends = 0;
    candidate = undefined;
    // Narrow edge: a slow earlier handler of this session event could let a run stamp messages
    // before this time; such a run then reports `idle`.
    watermark = Date.now();
    quietPolls = 0;
    lastEndContinues = false;
    approvals.clear();
    questions.clear();
    retrying = false;
    compacting = false;
    status = { state: ctx.isIdle() ? "idle" : "working" };
  };
  for (const event of ["session_start", "session_switch", "session_branch", "session_tree"]) {
    listen("main", event, startSession);
  }
  listen("main", "session_shutdown", () => {
    closed = true;
    socket?.end();
    socket = undefined;
  });

  listen("main", "agent_start", () => {
    // An end already delivered for this start (a slow earlier `agent_start` handler) means the run
    // is over: keep its settled status instead of reopening it.
    const ended = ends > starts;
    starts += 1;
    if (ended) return;
    quietPolls = 0;
    status = { state: "working" };
  });
  listen("main", "agent_end", (event, ctx) => {
    const messages = event.messages ?? [];
    const at = messages.at(-1)?.timestamp ?? 0;
    // Every end counts, even one delivered before its own `agent_start` (a slow earlier handler). A
    // late end of an earlier session also counts here; that only leaves the quiet check as barrier.
    ends += 1;
    lastEndContinues = event.willContinue === true;
    if (at > watermark && (!candidate || at >= candidate.at)) candidate = { messages, at };
    checkSettle(ctx, false);
  });
  listen("main", "auto_retry_start", () => (retrying = true));
  listen("main", "auto_retry_end", () => (retrying = false));
  listen("main", "auto_compaction_start", () => (compacting = true));
  listen("main", "auto_compaction_end", () => (compacting = false));

  listen("any", "tool_approval_requested", (event) => approvals.add(event.toolCallId ?? ""));
  listen("any", "tool_approval_resolved", (event) => approvals.delete(event.toolCallId ?? ""));
  listen("any", "tool_execution_start", (event) => {
    if (event.toolName === "ask") questions.add(event.toolCallId ?? "");
  });
  listen("any", "tool_execution_end", (event) => questions.delete(event.toolCallId ?? ""));
}
