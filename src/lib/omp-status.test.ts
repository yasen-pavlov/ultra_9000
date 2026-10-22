// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// This is an OMP-loaded plugin, not browser application code. Keep its Bun/Node type
// environment separate from the UI's ES2022/DOM project; the fixture supplies its host API.
interface Event {
  messages?: { role: string; stopReason: string; timestamp: number }[];
  willContinue?: boolean;
}
interface Context {
  agent: { kind: "main" };
  model: undefined;
  sessionManager: { getSessionId(): string; getSessionFile(): undefined; getCwd(): string };
  isIdle(): boolean;
  hasPendingMessages(): boolean;
  getAsyncJobSnapshot(): null;
  setInterval(callback: () => void, ms: number): unknown;
}
type Handler = (event: Event, ctx: Context) => void;
type Factory = (api: { on(event: string, handler: Handler): void; getSessionName(): undefined }) => void;

const wire = vi.hoisted(() => ({ lines: [] as string[] }));
vi.mock("node:fs", () => ({
  readFileSync: () => "0123456789abcdef0123456789abcdef",
  unlinkSync: () => undefined,
}));
vi.mock("node:net", () => ({
  createConnection: () => ({
    unref: () => undefined,
    on: () => undefined,
    write: (line: string) => wire.lines.push(line),
    end: () => undefined,
  }),
}));

let factory: Factory;

beforeEach(async () => {
  vi.useFakeTimers();
  vi.setSystemTime(1_000_000);
  wire.lines.length = 0;
  vi.stubEnv("ULTRA9000_OMP_LINK", "/test/status.sock");
  vi.stubEnv("ULTRA9000_OMP_TOKEN_FILE", "/test/token");
  // Loading the plugin after setting credentials exercises its module-load boundary.
  // Resolve at runtime so the browser type-checker does not import a Bun-hosted plugin.
  vi.resetModules();
  const path = new URL("../../src-tauri/omp/ultra9000-status.ts", import.meta.url).pathname;
  ({ default: factory } = (await import(path)) as { default: Factory });
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
    getAsyncJobSnapshot: () => null,
    setInterval: (callback, ms) => setInterval(callback, ms),
  };
  factory({
    on: (event, handler) => handlers.set(event, [...(handlers.get(event) ?? []), handler]),
    getSessionName: () => undefined,
  });
  const emit = (event: string, payload: Event = {}) => {
    for (const handler of handlers.get(event) ?? []) handler(payload, ctx);
  };
  return {
    emit,
    busy: () => (idle = false),
    idle: () => (idle = true),
    queueInput: () => (pending = true),
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
