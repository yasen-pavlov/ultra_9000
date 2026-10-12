// What the menu-bar popover does on keys and clicks (boards 05/06). Every action is a command to
// the app: the popover never decides order, waiting or confirmation itself. It only owns the
// keyboard focus ring, the inline close question and the "focus the new row" hand-off.
import { errorMessage, type CloseReason, type ItemState, type PopoverCommand, type PopoverReply } from "../lib/native";
import type { NewKind, PopoverKey } from "./keys";

/** One project of the list, in sidebar order (projects never fold here). */
export interface PopoverGroup {
  projectId: string;
  items: readonly ItemState[];
}

/** What the controller reads from the current snapshot. */
export interface PopoverView {
  groups: readonly PopoverGroup[];
  /** The item the main window shows. */
  selectedId: string | null;
  /** Needs-you plus unread your-turn items: what `j` jumps through. */
  waitingCount: number;
}

/** The native side of the popover; a seam so the behaviour is testable without IPC. */
export interface PopoverEffects {
  command(command: PopoverCommand): Promise<PopoverReply>;
  hide(): Promise<void>;
  toggleMain(): Promise<void>;
  /** Scroll the focus stop (a `data-rove` key) into view and give it DOM focus. */
  reveal(stop: string): void;
}

export interface CloseQuestionState {
  itemId: string;
  reason: CloseReason;
  /** The item's state when the app asked; if it changes before confirming, the app is asked again. */
  stamp: string;
}

/**
 * A place the focus ring can rest, keyed like its element's `data-rove`: every row
 * (`row:<item>`), and the + of a project without rows (`plus:<project>`). The latter is the only
 * keyboard route to start work in a pinned, empty project; headers of projects with rows stay
 * skipped (board 06), since their rows reach the same project with n/t/e and o.
 */
interface Stop {
  key: string;
  projectId: string;
  item: ItemState | null;
}

type OkReply = Extract<PopoverReply, { ok: true }>;

export function rowStop(itemId: string): string {
  return `row:${itemId}`;
}

function closeStamp(item: ItemState): string {
  const job = item.foregroundJob;
  return `${item.activity}|${item.stateSince}|${job?.command ?? ""}|${job?.sinceMs ?? ""}`;
}

export class PopoverController {
  /** Stop holding the keyboard focus ring; null until the first key after opening. */
  focus = $state<string | null>(null);
  /** The open inline close question, if any. */
  question = $state<CloseQuestionState | null>(null);
  /** The last failed command's message, shown until the next action. */
  error = $state<string | null>(null);

  readonly #view: () => PopoverView;
  readonly #effects: PopoverEffects;
  /** Stops at the previous sync, so a vanished focus stop hands the ring to its neighbour. */
  #order: Stop[] = [];
  /** A row launched with n/t/e that should take focus once it appears. */
  #pendingNew: string | null = null;
  /** Bumped on every show and hide; replies that arrive in a later opening change nothing here. */
  #opening = 0;

  constructor(view: () => PopoverView, effects: PopoverEffects) {
    this.#view = view;
    this.#effects = effects;
  }

  /** Each native show: no ring until the first key, no stale question. */
  opened(): void {
    this.#opening += 1;
    this.focus = null;
    this.question = null;
    this.error = null;
    this.#pendingNew = null;
    this.#order = this.#stops();
  }

  /** Dismissed: a pending question is dropped and the item keeps running. */
  hidden(): void {
    this.#opening += 1;
    this.question = null;
    this.#pendingNew = null;
  }

  /** Where the ring starts on the first key, and the list's Tab entry: the item the main window
   *  shows, else the first needs-you row, else the first stop. */
  entry(): string | null {
    const stops = this.#stops();
    const { selectedId } = this.#view();
    const start =
      stops.find((stop) => stop.item !== null && stop.item.id === selectedId) ??
      stops.find((stop) => stop.item?.activity === "needs-you") ??
      stops[0];
    return start?.key ?? null;
  }

  /** Call after every snapshot. */
  sync(): void {
    const stops = this.#stops();
    const present = new Set(stops.map((stop) => stop.key));
    if (this.question !== null && !present.has(rowStop(this.question.itemId))) this.question = null;
    if (this.focus !== null && !present.has(this.focus)) this.focus = this.#successor(this.focus, stops, present);
    if (this.#pendingNew !== null && present.has(rowStop(this.#pendingNew))) {
      const id = this.#pendingNew;
      this.#pendingNew = null;
      this.#focus(rowStop(id));
    }
    this.#order = stops;
  }

  /** Handles one popover key; false when the key is not the popover's (let it through). OS key
   *  repeat only moves the ring: holding w, n or ↵ must not confirm, launch or select again. */
  key(key: PopoverKey, repeat = false): boolean {
    if (repeat && key.kind !== "move") return true;
    this.error = null;
    if (this.question !== null) {
      switch (key.kind) {
        case "close":
        case "enter":
          void this.confirm();
          return true;
        case "move":
        case "escape":
          this.cancel();
          return true;
        default:
          break;
      }
    }
    const stop = this.#focusedStop();
    switch (key.kind) {
      case "move":
        this.#move(key.to);
        return true;
      case "enter":
        if (stop === null) this.#move("start");
        else if (stop.item === null) void this.palette({ id: stop.projectId, mode: "new" });
        else void this.select(stop.item.id);
        return true;
      case "row": {
        const row = this.#view().groups.flatMap((group) => group.items)[key.index];
        if (row !== undefined) void this.select(row.id);
        return true;
      }
      case "jump":
        void this.jump();
        return true;
      case "palette":
        void this.palette();
        return true;
      case "window":
        void this.toggleMain();
        return true;
      case "new":
        void this.newItem(key.item);
        return true;
      case "close":
        if (stop === null) this.#move("start");
        else if (stop.item !== null) void this.close(stop.item.id);
        return true;
      case "escape":
        void this.#effects.hide();
        return true;
    }
  }

  /** Row click, Enter, 1–9: show the item in the main window; the popover closes. */
  async select(itemId: string): Promise<void> {
    if (typeof (await this.#reply({ type: "select_item", itemId, show: true })) === "object")
      await this.#effects.hide();
  }

  /** j / Jump: the app's waiting order; nothing waiting, nothing happens. */
  async jump(): Promise<void> {
    if (this.#view().waitingCount === 0) return;
    if (typeof (await this.#reply({ type: "jump_to_waiting" })) === "object") await this.#effects.hide();
  }

  /** o / palette button: scoped to the focused stop's project. A project's +: that project, in
   *  "new" mode like the sidebar's +. The palette lives in the main window; the popover closes. */
  async palette(project?: { id: string; mode: "new" }): Promise<void> {
    const scope = project?.id ?? this.#focusedStop()?.projectId;
    const command: Extract<PopoverCommand, { type: "open_palette" }> = { type: "open_palette" };
    if (scope !== undefined) command.projectId = scope;
    if (project !== undefined) command.mode = project.mode;
    if (typeof (await this.#reply(command)) === "object") await this.#effects.hide();
  }

  /** h / window button: the main window shows or hides behind; the popover stays open and key. */
  async toggleMain(): Promise<void> {
    const opening = this.#opening;
    try {
      await this.#effects.toggleMain();
    } catch (error) {
      if (this.#opening === opening) this.error = errorMessage(error);
    }
  }

  /** n/t/e: in the focused stop's project, else the launch default. Stays open; the new row
   *  takes focus. */
  async newItem(kind: NewKind): Promise<void> {
    const projectId = this.#focusedStop()?.projectId;
    const reply = await this.#reply(
      projectId === undefined ? { type: "new_item", kind } : { type: "new_item", projectId, kind },
    );
    if (typeof reply !== "object" || reply.result === "needs-confirmation" || reply.itemId === undefined) return;
    const key = rowStop(reply.itemId);
    if (this.#stops().some((stop) => stop.key === key)) this.#focus(key);
    else this.#pendingNew = reply.itemId;
  }

  /** w / ⌫ / row ×: close, or turn the row into the app's question. Stays open. */
  async close(itemId: string, confirmed = false): Promise<void> {
    const before = this.#item(itemId);
    if (before === undefined) return;
    const reply = await this.#reply({ type: "close_item", itemId, confirmed });
    if (reply === "stale") return;
    if (reply === "failed" || reply.result !== "needs-confirmation") {
      this.question = null;
      return;
    }
    const current = this.#item(itemId) ?? before;
    this.question = { itemId, reason: reply.reason, stamp: closeStamp(current) };
    this.focus = rowStop(itemId);
  }

  /** End in the question: confirmed if the item is unchanged, else the app is asked again. */
  async confirm(): Promise<void> {
    const question = this.question;
    if (question === null) return;
    const item = this.#item(question.itemId);
    if (item === undefined) {
      this.question = null;
      return;
    }
    await this.close(item.id, closeStamp(item) === question.stamp);
  }

  /** Keep running: the question goes, the ring returns to its row. */
  cancel(): void {
    const question = this.question;
    if (question === null) return;
    this.question = null;
    this.#focus(rowStop(question.itemId));
  }

  /** Edge chip: scroll the row into view and put the ring on it, without selecting it. */
  reveal(itemId: string): void {
    this.question = null;
    this.#focus(rowStop(itemId));
  }

  /** DOM focus landed on a stop (Tab into the list, a click on a row's ×); other keys are ignored. */
  focused(stop: string): void {
    if (this.#stops().some((entry) => entry.key === stop)) this.focus = stop;
  }

  #stops(): Stop[] {
    return this.#view().groups.flatMap((group): Stop[] =>
      group.items.length === 0
        ? [{ key: `plus:${group.projectId}`, projectId: group.projectId, item: null }]
        : group.items.map((item) => ({ key: rowStop(item.id), projectId: group.projectId, item })),
    );
  }

  #item(itemId: string): ItemState | undefined {
    for (const group of this.#view().groups) {
      const item = group.items.find((entry) => entry.id === itemId);
      if (item !== undefined) return item;
    }
    return undefined;
  }

  #focusedStop(): Stop | null {
    return this.focus === null ? null : (this.#stops().find((stop) => stop.key === this.focus) ?? null);
  }

  /** The ring's next home when its stop vanished: an empty project's + hands it to that project's
   *  first row; a row to the next stop that remains, else the previous one. */
  #successor(gone: string, stops: Stop[], present: ReadonlySet<string>): string | null {
    const at = this.#order.findIndex((stop) => stop.key === gone);
    if (at < 0) return null;
    const old = this.#order[at];
    if (old.item === null) {
      const sameProject = stops.find((stop) => stop.projectId === old.projectId);
      if (sameProject !== undefined) return sameProject.key;
    }
    const after = this.#order.slice(at + 1).find((stop) => present.has(stop.key));
    const before = this.#order
      .slice(0, at)
      .reverse()
      .find((stop) => present.has(stop.key));
    return (after ?? before)?.key ?? null;
  }

  #focus(stop: string): void {
    this.focus = stop;
    this.#effects.reveal(stop);
  }

  /** Arrows/Home/End through stops, no wrap; the first key goes to `entry()`. */
  #move(to: "up" | "down" | "home" | "end" | "start"): void {
    const stops = this.#stops();
    if (stops.length === 0) return;
    const at = this.focus === null ? -1 : stops.findIndex((stop) => stop.key === this.focus);
    let next: number;
    if (to === "home") next = 0;
    else if (to === "end") next = stops.length - 1;
    else if (at < 0)
      next = Math.max(
        0,
        stops.findIndex((stop) => stop.key === this.entry()),
      );
    else if (to === "up") next = Math.max(0, at - 1);
    else if (to === "down") next = Math.min(stops.length - 1, at + 1);
    else next = at;
    this.#focus(stops[next].key);
  }

  /** The ok reply; "failed" after recording why; "stale" when the popover was hidden or reopened
   *  meanwhile (the command still ran in the app, but this opening must not change because of it). */
  async #reply(command: PopoverCommand): Promise<OkReply | "failed" | "stale"> {
    const opening = this.#opening;
    let failure: string;
    try {
      const reply = await this.#effects.command(command);
      if (this.#opening !== opening) return "stale";
      if (reply.ok) return reply;
      failure = reply.message;
    } catch (error) {
      if (this.#opening !== opening) return "stale";
      failure = errorMessage(error);
    }
    this.error = failure;
    return "failed";
  }
}
