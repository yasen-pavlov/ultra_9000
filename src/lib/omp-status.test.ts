// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// This is an OMP-loaded plugin, not browser application code. Keep its Bun/Node type
// environment separate from the UI's ES2022/DOM project; the fixture supplies its host API.
interface Event {
  messages?: { role: string; stopReason: string; timestamp: number }[];
  willContinue?: boolean;
  toolCallId?: string;
  toolName?: string;
}
interface AsyncJob {
  type: string;
  label: string;
  startTime: number;
}
interface Context {
  agent: { kind: "main" | "sub" };
  model: undefined;
  sessionManager: { getSessionId(): string; getSessionFile(): undefined; getCwd(): string };
  isIdle(): boolean;
  hasPendingMessages(): boolean;
  getAsyncJobSnapshot(): { running: AsyncJob[]; delivery: { queued: number; delivering: boolean } } | null;
  setInterval(callback: () => void, ms: number): unknown;
}
type Handler = (event: Event, ctx: Context) => void;
type Factory = (api: { on(event: string, handler: Handler): void; getSessionName(): undefined }) => void;

const TOKEN = "0123456789abcdef0123456789abcdef";
const SOCKET = "/test/status.sock";
const TOKEN_FILE = "/test/token";

// The file system as the extension sees it: the app's socket and its item's token file.
const fs = vi.hoisted(() => ({
  files: new Map<string, { data: string; mode?: number }>(),
  /** Paths whose deletion fails. */
  undeletable: new Set<string>(),
}));
vi.mock("node:fs", () => ({
  existsSync: (path: string) => fs.files.has(path),
  readFileSync: (path: string) => {
    const file = fs.files.get(path);
    if (file === undefined) throw new Error(`ENOENT: ${path}`);
    return file.data;
  },
  unlinkSync: (path: string) => {
    if (fs.undeletable.has(path)) throw new Error(`EACCES: ${path}`);
    if (!fs.files.delete(path)) throw new Error(`ENOENT: ${path}`);
  },
  writeFileSync: (path: string, data: string, options: { mode: number; flag: string }) => {
    if (options.flag === "wx" && fs.files.has(path)) throw new Error(`EEXIST: ${path}`);
    fs.files.set(path, { data, mode: options.mode });
  },
}));
const wire = vi.hoisted(() => ({
  lines: [] as string[],
  ended: 0,
  /** Closes the newest connection from the app's side. */
  close: (): void => undefined,
}));
vi.mock("node:net", () => ({
  createConnection: () => {
    const listeners = new Map<string, () => void>();
    wire.close = () => listeners.get("close")?.();
    return {
      unref: () => undefined,
      on: (event: string, listener: () => void) => listeners.set(event, listener),
      destroy: () => undefined,
      write: (line: string) => wire.lines.push(line),
      end: () => (wire.ended += 1),
    };
  },
}));

let factory: Factory;

/** Loads the plugin like a fresh OMP image: the native environment names the link again. */
async function load(): Promise<void> {
  vi.stubEnv("ULTRA9000_OMP_LINK", SOCKET);
  vi.stubEnv("ULTRA9000_OMP_TOKEN_FILE", TOKEN_FILE);
  // Resolve at runtime so the browser type-checker does not import a Bun-hosted plugin.
  vi.resetModules();
  const path = new URL("../../src-tauri/omp/ultra9000-status.ts", import.meta.url).pathname;
  ({ default: factory } = (await import(path)) as { default: Factory });
}

beforeEach(async () => {
  vi.useFakeTimers();
  vi.setSystemTime(1_000_000);
  wire.lines.length = 0;
  wire.ended = 0;
  fs.files.clear();
  fs.undeletable.clear();
  fs.files.set(SOCKET, { data: "" });
  fs.files.set(TOKEN_FILE, { data: `${TOKEN}\n` });
  // Loading the plugin after setting credentials exercises its module-load boundary.
  await load();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
});
function reporter() {
  const handlers = new Map<string, Handler[]>();
  let idle = true;
  let session = "original";
  let pending = false;
  const jobs: AsyncJob[] = [];
  const ctx: Context = {
    agent: { kind: "main" },
    model: undefined,
    sessionManager: {
      getSessionId: () => session,
      getSessionFile: () => undefined,
      getCwd: () => "/project",
    },
    isIdle: () => idle,
    hasPendingMessages: () => pending,
    getAsyncJobSnapshot: () =>
      jobs.length > 0 ? { running: [...jobs], delivery: { queued: 0, delivering: false } } : null,
    setInterval: (callback, ms) => setInterval(callback, ms),
  };
  factory({
    on: (event, handler) => handlers.set(event, [...(handlers.get(event) ?? []), handler]),
    getSessionName: () => undefined,
  });
  const emit = (event: string, payload: Event = {}, agent: Context = ctx) => {
    for (const handler of handlers.get(event) ?? []) handler(payload, agent);
  };
  return {
    emit,
    /** The same event from an in-process subagent. */
    emitSub: (event: string, payload: Event = {}) => emit(event, payload, { ...ctx, agent: { kind: "sub" } }),
    busy: () => (idle = false),
    idle: () => (idle = true),
    queueInput: () => (pending = true),
    startJob: (label: string, startTime: number) => jobs.push({ type: "bash", label, startTime }),
    endJobs: () => (jobs.length = 0),
    switchSession: () => {
      session = "replacement";
      emit("session_switch");
    },
  };
}

function lastStatus(): unknown {
  return wire.lines
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .reverse()
    .find((line) => line.type === "state");
}

function endAt(timestamp: number): Event {
  return { messages: [{ role: "assistant", stopReason: "stop", timestamp }] };
}

describe("OMP status with delayed lifecycle notifications", () => {
  it("reports a new active run despite an earlier session's late end, and settles the new outcome", () => {
    const agent = reporter();
    agent.emit("session_start");
    agent.busy();
    agent.emit("agent_start");
    agent.idle();
    agent.switchSession();
    agent.emit("agent_end", endAt(999_000));
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });

    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(15_000);
    expect(lastStatus()).toEqual({ type: "state", state: "working" });

    agent.idle();
    // A later turn must not settle before its own delayed end notification.
    vi.advanceTimersByTime(500);
    expect(lastStatus()).toEqual({ type: "state", state: "working" });
    agent.emit("agent_end", endAt(Date.now()));
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
  });

  it("keeps a completed outcome when its own start notification arrives after completion", () => {
    const agent = reporter();
    agent.emit("session_start");
    vi.advanceTimersByTime(1_000);
    agent.emit("agent_end", endAt(Date.now()));
    expect(lastStatus()).toEqual({ type: "state", state: "done" });

    // Prompt preflight can be in flight without having started another agent run.
    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(15_000);
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
  });

  it("still counts an older end from this session after a newer outcome has already settled", () => {
    const agent = reporter();
    agent.emit("session_start");
    vi.advanceTimersByTime(1_000);
    const earlierRunEnd = endAt(Date.now());

    // The earlier run's start/end notifications are both delayed behind a newer run.
    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(1_000);
    agent.idle();
    agent.emit("agent_end", endAt(Date.now()));
    expect(lastStatus()).toEqual({ type: "state", state: "done" });

    // This is older than the outcome watermark, but newer than the session boundary.
    agent.emit("agent_end", earlierRunEnd);
    agent.busy();
    agent.emit("agent_start");
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
  });

  it("recovers counter balance after both notifications from an old session arrive late", () => {
    const agent = reporter();
    agent.emit("session_start");
    agent.switchSession();
    agent.emit("agent_start");
    agent.emit("agent_end", endAt(999_000));
    vi.advanceTimersByTime(10_000);
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });

    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(1_000);
    expect(lastStatus()).toEqual({ type: "state", state: "working" });
    agent.idle();
    agent.emit("agent_end", endAt(Date.now()));
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
  });

  it("accepts a late outcome after fallback without counting the abandoned run against the next start", () => {
    const agent = reporter();
    agent.emit("session_start");
    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(1_000);
    agent.idle();
    const delayedEnd = endAt(Date.now());
    vi.advanceTimersByTime(10_000);
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });

    agent.emit("agent_end", delayedEnd);
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
    agent.busy();
    agent.emit("agent_start");
    expect(lastStatus()).toEqual({ type: "state", state: "working" });
    vi.advanceTimersByTime(1_000);
    agent.idle();
    agent.emit("agent_end", endAt(Date.now()));
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
  });

  it("does not announce an earlier run's pending outcome after fallback while the latest end is held", () => {
    const agent = reporter();
    agent.emit("session_start");
    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(1_000);
    const earlierEnd = endAt(Date.now());
    agent.emit("agent_start");
    agent.emit("agent_end", earlierEnd);
    vi.advanceTimersByTime(1_000);
    agent.idle();
    const latestEnd = endAt(Date.now());
    vi.advanceTimersByTime(11_000);
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });

    agent.emit("agent_end", latestEnd);
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
  });

  it("does not let an abandoned continuation override a newer terminal end with blocked queued input", () => {
    const agent = reporter();
    agent.emit("session_start");
    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(1_000);
    agent.idle();
    const abandonedEnd = { ...endAt(Date.now()), willContinue: true };
    vi.advanceTimersByTime(10_000);
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });

    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(1_000);
    agent.emit("agent_end", endAt(Date.now()));
    agent.emit("agent_end", abandonedEnd);
    agent.queueInput();
    agent.idle();
    vi.advanceTimersByTime(500);
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
  });
});

function hellos(): Record<string, unknown>[] {
  return wire.lines.map((line) => JSON.parse(line) as Record<string, unknown>).filter((line) => line.type === "hello");
}

describe("OMP status link across a re-executed OMP", () => {
  it("writes its token back at shutdown, and the next image reads it, deletes it and says hello with it", async () => {
    const agent = reporter();
    agent.emit("session_start");
    expect(fs.files.has(TOKEN_FILE)).toBe(false);
    expect(hellos()).toEqual([{ v: 1, type: "hello", token: TOKEN, omp: "unknown" }]);

    agent.emit("session_shutdown");
    expect(wire.ended).toBe(1);
    expect(fs.files.get(TOKEN_FILE)).toEqual({ data: TOKEN, mode: 0o600 });
    // The shutting-down image never reconnects: that would end the next image's token file.
    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(5_000);
    expect(hellos()).toHaveLength(1);

    // `/restart` re-executes the process in place with the same native environment.
    await load();
    expect(fs.files.has(TOKEN_FILE)).toBe(false);
    const restarted = reporter();
    restarted.emit("session_start");
    expect(hellos()).toEqual([
      { v: 1, type: "hello", token: TOKEN, omp: "unknown" },
      { v: 1, type: "hello", token: TOKEN, omp: "unknown" },
    ]);
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });
  });

  it("writes nothing without a usable token", async () => {
    for (const token of [undefined, "not a token"]) {
      fs.files.delete(TOKEN_FILE);
      if (token !== undefined) fs.files.set(TOKEN_FILE, { data: token });
      await load();
      expect(fs.files.has(TOKEN_FILE)).toBe(false);
      const agent = reporter();
      agent.emit("session_start");
      agent.emit("session_shutdown");
      expect(fs.files.has(TOKEN_FILE)).toBe(false);
      expect(wire.lines).toEqual([]);
    }
  });

  it("leaves no token behind once the app has quit", () => {
    const agent = reporter();
    agent.emit("session_start");
    // The app removes its socket before it deletes the token files it knows of.
    fs.files.delete(SOCKET);
    agent.emit("session_shutdown");
    expect(fs.files.has(TOKEN_FILE)).toBe(false);
  });

  it("never replaces another file at the token path", () => {
    const agent = reporter();
    agent.emit("session_start");
    fs.files.set(TOKEN_FILE, { data: "someone else's" });
    agent.emit("session_shutdown");
    expect(fs.files.get(TOKEN_FILE)).toEqual({ data: "someone else's" });
    expect(wire.ended).toBe(1);
  });

  it("reports the turn a restarted image resumes", async () => {
    const agent = reporter();
    agent.emit("session_start");
    agent.emit("session_shutdown");
    // An image that starts busy reports working from its startup ...
    await load();
    const busy = reporter();
    busy.busy();
    busy.emit("session_start");
    expect(lastStatus()).toEqual({ type: "state", state: "working" });
    busy.emit("session_shutdown");
    // ... and one that starts idle reports the interrupted turn its restore retries.
    await load();
    const restored = reporter();
    restored.emit("session_start");
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });
    restored.busy();
    restored.emit("agent_start");
    expect(lastStatus()).toEqual({ type: "state", state: "working" });
    expect(hellos()).toHaveLength(3);
  });

  it("links with a token it read but could not delete", async () => {
    fs.files.set(TOKEN_FILE, { data: TOKEN });
    fs.undeletable.add(TOKEN_FILE);
    await load();
    reporter().emit("session_start");
    expect(hellos()).toEqual([{ v: 1, type: "hello", token: TOKEN, omp: "unknown" }]);
    // The app deletes it when the item ends.
    expect(fs.files.has(TOKEN_FILE)).toBe(true);
  });
});

describe("OMP status link reporting", () => {
  it("reconnects after the app dropped the connection and repeats what it knows", () => {
    const agent = reporter();
    agent.emit("session_start");
    wire.close();
    agent.busy();
    agent.emit("agent_start");
    // Not before the reconnect delay; then the poll reconnects and sends everything again.
    expect(hellos()).toHaveLength(1);
    const sent = wire.lines.length;
    vi.advanceTimersByTime(2_000);
    const resent = wire.lines.slice(sent).map((line) => JSON.parse(line) as Record<string, unknown>);
    expect(resent.map((line) => line.type)).toEqual(["hello", "session", "model", "name", "state"]);
    expect(lastStatus()).toEqual({ type: "state", state: "working" });
  });

  it("reports background jobs while the agent waits on them, then the outcome", () => {
    const agent = reporter();
    agent.emit("session_start");
    agent.busy();
    agent.emit("agent_start");
    vi.advanceTimersByTime(1_000);
    agent.startJob("cargo build", 5);
    agent.idle();
    agent.emit("agent_end", endAt(Date.now()));
    expect(lastStatus()).toEqual({
      type: "state",
      state: "waiting",
      jobs: [{ label: "cargo build", jobType: "bash", startedAt: 5 }],
    });
    agent.endJobs();
    vi.advanceTimersByTime(500);
    expect(lastStatus()).toEqual({ type: "state", state: "done" });
  });

  it("needs the user for a subagent's approval or question, never for its turns", () => {
    const agent = reporter();
    agent.emit("session_start");
    agent.emitSub("tool_approval_requested", { toolCallId: "a" });
    expect(lastStatus()).toEqual({ type: "state", state: "needs-you", kind: "approval" });
    agent.emitSub("tool_approval_resolved", { toolCallId: "a" });
    agent.emitSub("tool_execution_start", { toolCallId: "b", toolName: "bash" });
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });
    agent.emitSub("tool_execution_start", { toolCallId: "q", toolName: "ask" });
    expect(lastStatus()).toEqual({ type: "state", state: "needs-you", kind: "question" });
    agent.emitSub("tool_execution_end", { toolCallId: "q" });
    agent.emitSub("agent_start");
    expect(lastStatus()).toEqual({ type: "state", state: "idle" });
  });
});
