// Wire-shaped native snapshots for unit tests; every field defaults to a plausible running state.
import type {
  ConversationList,
  DocumentState,
  ItemState,
  ProjectState,
  SettingsState,
  ToolInfo,
  WorkspaceState,
} from "../lib/native";

export function itemState(overrides: Partial<ItemState> = {}): ItemState {
  return {
    id: "item-1",
    tool: "omp",
    kind: "agent",
    projectId: "/home/ana/w/atlas",
    cwd: "/home/ana/w/atlas",
    title: "Fix the parser",
    process: "running",
    document: null,
    exitCode: null,
    failure: null,
    activity: "unknown",
    attention: null,
    unread: false,
    conversation: null,
    model: null,
    startedAt: 0,
    stateSince: 0,
    pid: 4242,
    foregroundJob: null,
    backgroundJobs: [],
    ...overrides,
  };
}

export function documentState(overrides: Partial<DocumentState> = {}): DocumentState {
  return {
    path: "/home/ana/w/atlas/docs/reviews/importer-rollover.md",
    dir: "docs/reviews",
    status: "ok",
    error: null,
    revision: 1,
    lastEvent: "opened",
    lastEventAtMs: 0,
    modifiedMs: 0,
    sizeBytes: 1024,
    liveUntilMs: 0,
    follow: false,
    ...overrides,
  };
}

/** A document row exactly as native sends it (contract decision 2): no process, no status. */
export function documentItem(overrides: Partial<ItemState> = {}, doc: Partial<DocumentState> = {}): ItemState {
  const document = documentState(doc);
  const slash = document.path.lastIndexOf("/");
  return itemState({
    id: "doc-1",
    tool: "document",
    kind: "document",
    cwd: document.path.slice(0, slash),
    title: document.path.slice(slash + 1),
    process: null,
    document,
    pid: null,
    ...overrides,
  });
}

export function toolInfo(overrides: Partial<ToolInfo> = {}): ToolInfo {
  return {
    id: "omp",
    label: "OMP",
    glyph: "",
    kind: "agent",
    status: "omp",
    canResume: true,
    available: true,
    path: null,
    unavailableReason: null,
    icon: null,
    accent: null,
    ...overrides,
  };
}

export function projectState(overrides: Partial<ProjectState> = {}): ProjectState {
  const path = overrides.path ?? overrides.id ?? "/home/ana/w/atlas";
  return {
    id: path,
    name: path.slice(path.lastIndexOf("/") + 1),
    path,
    pinned: false,
    missing: false,
    defaultTool: "omp",
    itemIds: [],
    ...overrides,
  };
}

export function workspaceState(overrides: Partial<WorkspaceState> = {}): WorkspaceState {
  return {
    revision: 1,
    presentationEpoch: 1,
    view: "terminal",
    sheet: null,
    projects: [],
    items: [],
    activeItemId: null,
    recentItemIds: [],
    tools: [],
    providerIcons: {},
    notices: [],
    lastSet: null,
    ui: { sidebarExpanded: true, foldedProjectIds: [] },
    shortcuts: [],
    defaultCwd: "/home/ana",
    hostError: null,
    ...overrides,
  };
}

export function settingsState(overrides: Partial<SettingsState> = {}): SettingsState {
  return {
    revision: 1,
    configDir: "/home/ana/.config/ultra9000",
    machine: "atlas-box",
    files: [],
    values: [],
    errors: [],
    reloadedAt: 0,
    ...overrides,
  };
}

export function conversationList(overrides: Partial<ConversationList> = {}): ConversationList {
  return { revision: 1, conversations: [], errors: [], scanning: false, ...overrides };
}

/** Resolves after every already-queued promise reaction (mocked IPC settles in microtasks). */
export function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

export interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason: unknown): void;
}

/**
 * A promise the test settles explicitly, to hold a native reply in flight. (Executor form: the
 * project's ES2022 lib, matching the Safari 16 web view target, has no Promise.withResolvers.)
 */
export function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
