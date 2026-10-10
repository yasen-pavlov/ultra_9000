// Typed bindings for the native commands and events (native/UI contract v2; the Rust serde types in
// src-tauri are the source of truth).
// This is the only IPC module. Rendering and terminal input never pass through here; the web view
// renders native snapshots and issues commands, nothing more.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type { UnlistenFn };

// ---------------------------------------------------------------------------------------------
// Wire types (frozen; keep identical to the Rust serde types)
// ---------------------------------------------------------------------------------------------

export type ItemKind = "agent" | "terminal";
export type ProcessState = "starting" | "running" | "exited" | "failed";
export type Activity = "unknown" | "working" | "needs-you" | "your-turn" | "on-job" | "error";
export type Attention = "approval" | "question" | null;

export interface ConversationRef {
  tool: string;
  id: string;
  path: string;
}

/** A background job an agent waits on while on job (OMP `jobType` → `kind`, e.g. "bash", "task"). */
export interface BackgroundJob {
  kind: string;
  label: string;
  startedAtMs: number;
}

export interface ItemState {
  id: string;
  tool: string;
  kind: ItemKind;
  projectId: string;
  /** Launch directory (recorded directory for resumes); not the live shell cwd. */
  cwd: string;
  /** Display title, never blank. */
  title: string;
  process: ProcessState;
  exitCode: number | null;
  failure: string | null;
  activity: Activity;
  attention: Attention;
  unread: boolean;
  conversation: ConversationRef | null;
  model: string | null;
  startedAt: number;
  stateSince: number;
  /** Display only; never signalled. */
  pid: number | null;
  /** Shell items only: the job holding the terminal (`cargo`), null at the prompt. Polled ~1 s, debounced. */
  foregroundJob: { command: string; sinceMs: number } | null;
  /** Non-empty only while `activity === "on-job"`; oldest first. */
  backgroundJobs: BackgroundJob[];
}

export interface ProjectState {
  id: string;
  name: string;
  path: string;
  pinned: boolean;
  missing: boolean;
  defaultTool: string;
  itemIds: string[];
}

export interface ToolInfo {
  /** Tool id, referenced by every `tool` field. Built-ins: "omp" | "claude" | "codex" | "pi" | "shell" | "neovim"; more via [tools.*]. */
  id: string;
  label: string;
  /** Configured glyph letters; "" when none (the generic agent mark is drawn instead). */
  glyph: string;
  kind: ItemKind;
  status: "omp" | "process";
  canResume: boolean;
  /** null while the login-shell PATH probe runs. */
  available: boolean | null;
  path: string | null;
  unavailableReason: string | null;
  /** Validated `[tools.<id>] icon` file; null = none (built-in mark, then glyph, then agent mark). */
  icon: ToolIcon | null;
  /** `#rrggbb` colour of a single-colour mark where colour shows; null = none. */
  accent: string | null;
}

export interface ToolIcon {
  /** data:image/svg+xml;base64,… or data:image/png;base64,… */
  url: string;
  tint: "mono" | "original";
  /** true: an opaque square logo fills the tile; false: contained in the mark box. */
  fill: boolean;
}

export type CloseReason = "agent-working" | "agent-needs-you" | "foreground-job" | "neovim" | "unmonitored-agent";

export type Sheet =
  | { kind: "palette"; projectId: string | null; mode: "open" | "new" }
  | { kind: "settings" }
  | { kind: "shortcuts" }
  | { kind: "confirm-close"; itemId: string; reason: CloseReason; detail: string | null }
  | { kind: "quit"; itemIds: string[] };

/** Sheets the web view may open itself; confirmations are opened by native only. */
export type OpenableSheet =
  { kind: "palette"; projectId: string | null; mode: "open" | "new" } | { kind: "settings" } | { kind: "shortcuts" };

export interface Notice {
  id: string;
  kind: "ended" | "error";
  title: string;
  body: string;
  projectId: string | null;
  conversation: ConversationRef | null;
  at: number;
}

export interface LastSetEntry {
  tool: string;
  title: string;
  projectId: string;
  cwd: string;
  conversation: ConversationRef;
}

export type ShortcutAction =
  | "palette"
  | "palette-project"
  | "new-agent"
  | "new-terminal"
  | "new-neovim"
  | "close"
  | "jump-waiting"
  | "item-1"
  | "item-2"
  | "item-3"
  | "item-4"
  | "item-5"
  | "item-6"
  | "item-7"
  | "item-8"
  | "item-9"
  | "item-previous"
  | "item-next"
  | "project-previous"
  | "project-next"
  | "focus-sidebar"
  | "toggle-sidebar"
  | "settings"
  | "shortcuts"
  | "hide"
  | "quit";

export interface Shortcut {
  action: ShortcutAction;
  /** Label for this platform, e.g. "Ctrl+Shift+Space"; null if unbound. */
  label: string | null;
}

export interface UiState {
  sidebarExpanded: boolean;
  foldedProjectIds: string[];
}

export interface WorkspaceState {
  revision: number;
  presentationEpoch: number;
  /** "sheet" ⇔ sheet !== null; the terminal is hidden. */
  view: "terminal" | "sheet";
  sheet: Sheet | null;
  /** Sidebar order: pinned order, then order of first running item; only pinned or non-empty. */
  projects: ProjectState[];
  items: ItemState[];
  activeItemId: string | null;
  /** MRU, most recent first, running/exited items only. */
  recentItemIds: string[];
  tools: ToolInfo[];
  /** Provider id → data URL, for valid `[providers.<id>] icon` overrides only. */
  providerIcons: Record<string, string>;
  /** Newest last; at most 5. */
  notices: Notice[];
  lastSet: LastSetEntry[] | null;
  ui: UiState;
  shortcuts: Shortcut[];
  /** Home directory. */
  defaultCwd: string;
  /** Terminal host failed to start; launches will fail. */
  hostError: string | null;
}

export interface ViewportRect {
  x: number;
  y: number;
  width: number;
  height: number;
  /** CSS px radius of the native view's top-left corner (the content panel's); 0 = square. */
  topLeftRadius: number;
  /** CSS px radius of the native view's bottom-left corner (the content panel's); 0 = square. */
  bottomLeftRadius: number;
}

export interface Conversation {
  tool: string;
  id: string;
  path: string;
  title: string;
  projectId: string;
  cwd: string;
  cwdMissing: boolean;
  createdAt: number;
  updatedAt: number;
  model: string | null;
  runningItemId: string | null;
  maybeOpenElsewhere: boolean;
}

export interface ConversationStoreError {
  tool: string;
  path: string;
  message: string;
}

export interface ConversationList {
  revision: number;
  /** updatedAt descending. */
  conversations: Conversation[];
  errors: ConversationStoreError[];
  /** First scan not finished. */
  scanning: boolean;
}

export type SettingsKey =
  | "launch.default_tool"
  | "sidebar.width"
  | "sidebar.density"
  | "sidebar.provider_mark"
  | "sidebar.auto_collapse_below"
  | "layout.context_bar"
  | "icons.brand_color"
  | "tray.icon"
  | "notify.needs_you"
  | "notify.turn_finished"
  | "terminals.remove_on_clean_exit"
  | "terminals.confirm_close_with_foreground_job"
  | "confirm.close_unmonitored_agents"
  | "resume.open_elsewhere_hint_seconds"
  | "projects.pinned"
  | "terminal.font.size";

export type SettingScalar = string | number | boolean | string[];

export interface SettingValue {
  /** SettingsKey, or `projects."<path>".default_tool`. */
  key: string;
  value: SettingScalar;
  /** null = built-in default. */
  source: { file: string; line: number } | null;
  defaultValue: SettingScalar;
}

export interface SettingsError {
  file: string;
  line: number | null;
  key: string | null;
  message: string;
  lastGood: string | null;
}

export interface SettingsFile {
  name: string;
  path: string;
  exists: boolean;
}

export interface SettingsState {
  revision: number;
  configDir: string;
  machine: string;
  files: SettingsFile[];
  values: SettingValue[];
  errors: SettingsError[];
  reloadedAt: number;
}

export interface PathCheck {
  normalized: string | null;
  error: string | null;
  suggestion: string | null;
}

export type WriteTarget = "auto" | "machine" | { project: string };

export type CloseOutcome = "closed" | "confirm";

export interface ConversationsChanged {
  revision: number;
}

export interface UiCommand {
  command: "focus-sidebar";
}

// ---------------------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------------------

export const WORKSPACE_STATE_EVENT = "workspace-state";
export const SETTINGS_STATE_EVENT = "settings-state";
export const CONVERSATIONS_CHANGED_EVENT = "conversations-changed";
export const UI_COMMAND_EVENT = "ui-command";

export function listenWorkspaceState(handler: (state: WorkspaceState) => void): Promise<UnlistenFn> {
  return listen<WorkspaceState>(WORKSPACE_STATE_EVENT, (event) => handler(event.payload));
}

export function listenSettingsState(handler: (state: SettingsState) => void): Promise<UnlistenFn> {
  return listen<SettingsState>(SETTINGS_STATE_EVENT, (event) => handler(event.payload));
}

export function listenConversationsChanged(handler: (change: ConversationsChanged) => void): Promise<UnlistenFn> {
  return listen<ConversationsChanged>(CONVERSATIONS_CHANGED_EVENT, (event) => handler(event.payload));
}

export function listenUiCommand(handler: (command: UiCommand) => void): Promise<UnlistenFn> {
  return listen<UiCommand>(UI_COMMAND_EVENT, (event) => handler(event.payload));
}

/** Key-window changes of this app window (not first-responder changes inside it). */
export function listenWindowFocus(handler: (focused: boolean) => void): Promise<UnlistenFn> {
  return getCurrentWindow().onFocusChanged((event) => handler(event.payload));
}

// ---------------------------------------------------------------------------------------------
// Commands (one wrapper per contract command)
// ---------------------------------------------------------------------------------------------

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "message" in error && typeof error.message === "string") {
    return error.message;
  }
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

export function fetchWorkspaceState(): Promise<WorkspaceState> {
  return invoke<WorkspaceState>("workspace_state");
}

/** Creates the row at once (process "starting"), selects it and closes any sheet; resolves its id. */
export function itemLaunch(tool: string, cwd: string, anchorItemId?: string): Promise<string> {
  return invoke<string>("item_launch", anchorItemId === undefined ? { tool, cwd } : { tool, cwd, anchorItemId });
}

/** Selects the item that already has this conversation, otherwise resumes it in its recorded cwd. */
export function itemResume(tool: string, conversationId: string, path: string): Promise<string> {
  return invoke<string>("item_resume", { tool, conversationId, path });
}

/** For exited/failed rows; the new row replaces the old one in place. */
export function itemRestart(itemId: string): Promise<string> {
  return invoke<string>("item_restart", { itemId });
}

export function itemSelect(itemId: string): Promise<void> {
  return invoke<void>("item_select", { itemId });
}

/** "confirm": native opened the confirm-close sheet instead of ending the process. */
export function itemClose(itemId: string, confirmed: boolean): Promise<CloseOutcome> {
  return invoke<CloseOutcome>("item_close", { itemId, confirmed });
}

export function noticeDismiss(noticeId: string): Promise<void> {
  return invoke<void>("notice_dismiss", { noticeId });
}

export function sheetOpen(sheet: OpenableSheet): Promise<void> {
  return invoke<void>("sheet_open", { sheet });
}

/** Back to the active item's terminal; closing confirm-close cancels it. */
export function sheetClose(): Promise<void> {
  return invoke<void>("sheet_close");
}

export function appHide(): Promise<void> {
  return invoke<void>("app_hide");
}

export function appRequestQuit(): Promise<void> {
  return invoke<void>("app_request_quit");
}

/** Confirmed from the quit sheet. */
export function appQuit(): Promise<void> {
  return invoke<void>("app_quit");
}

export function lastSetResume(): Promise<void> {
  return invoke<void>("last_set_resume");
}

export function lastSetDismiss(): Promise<void> {
  return invoke<void>("last_set_dismiss");
}

export function uiSet(change: { sidebarExpanded?: boolean; foldedProjectIds?: string[] }): Promise<void> {
  return invoke<void>("ui_set", change);
}

/** Same validation as launch, as data (never a shell). */
export function pathCheck(path: string): Promise<PathCheck> {
  return invoke<PathCheck>("path_check", { path });
}

/** Native folder picker; null on cancel. */
export function pickDirectory(initialDirectory: string): Promise<string | null> {
  return invoke<string | null>("pick_directory", { initialDirectory });
}

export function conversationsList(
  filter: { projectId?: string; query?: string; limit?: number } = {},
): Promise<ConversationList> {
  return invoke<ConversationList>("conversations_list", filter);
}

export function fetchSettingsState(): Promise<SettingsState> {
  return invoke<SettingsState>("settings_state");
}

/** Comment-preserving write, then reload; rejects invalid values without writing. */
export function settingsSet(key: string, value: SettingScalar, target: WriteTarget): Promise<void> {
  return invoke<void>("settings_set", { key, value, target });
}

/** Removes the key from the target file so the default applies. */
export function settingsReset(key: string, target: WriteTarget): Promise<void> {
  return invoke<void>("settings_reset", { key, target });
}

/** Opens the file in a new Neovim item in the config-dir project. */
export function settingsOpenFile(file: string, line?: number): Promise<void> {
  return invoke<void>("settings_open_file", line === undefined ? { file } : { file, line });
}

export function settingsReveal(): Promise<void> {
  return invoke<void>("settings_reveal");
}

/** True only when native applied the report for its current selection and presentation epoch. */
export function setTerminalViewport(
  itemId: string,
  presentationEpoch: number,
  rect: ViewportRect,
  visible: boolean,
  sequence: number,
): Promise<boolean> {
  return invoke<boolean>("terminal_set_viewport", { itemId, presentationEpoch, rect, visible, sequence });
}

/** True only for the current visible item and its exact successfully applied report sequence. */
export function focusTerminal(itemId: string, presentationEpoch: number, sequence: number): Promise<boolean> {
  return invoke<boolean>("terminal_focus", { itemId, presentationEpoch, sequence });
}
