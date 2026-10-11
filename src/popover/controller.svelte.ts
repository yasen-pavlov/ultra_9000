// What the menu-bar popover does on keys and clicks (boards 05/06). Every action is a command to
// the app: the popover never decides order, waiting or confirmation itself. It only owns the
// keyboard focus ring, the inline close question and the "focus the new row" hand-off.
import { errorMessage, type CloseReason, type ItemState, type PopoverCommand, type PopoverReply } from "../lib/native";
import type { NewKind, PopoverKey } from "./keys";

/** What the controller reads from the current snapshot. */
export interface PopoverView {
  /** Every live row, in sidebar order (projects never fold here). */
  rows: readonly ItemState[];
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
  /** Scroll the row into view (and move DOM focus to it while the ring shows). */
  reveal(itemId: string): void;
}

export interface CloseQuestionState {
  itemId: string;
  reason: CloseReason;
  /** The item's state when the app asked; if it changes before confirming, the app is asked again. */
  stamp: string;
}

function closeStamp(item: ItemState): string {
  return `${item.activity}|${item.stateSince}|${item.foregroundJob?.command ?? ""}`;
}

export class PopoverController {
  /** Row holding the keyboard focus ring; null until the first key after opening. */
  focusId = $state<string | null>(null);
  /** The open inline close question, if any. */
  question = $state<CloseQuestionState | null>(null);
  /** The last failed command's message, shown until the next action. */
  error = $state<string | null>(null);

  readonly #view: () => PopoverView;
  readonly #effects: PopoverEffects;
  /** Row order at the previous sync, so a vanished focused row hands the ring to its neighbour. */
  #order: string[] = [];
  /** A row launched with n/t/e that should take focus once it appears. */
  #pendingNew: string | null = null;

  constructor(view: () => PopoverView, effects: PopoverEffects) {
    this.#view = view;
    this.#effects = effects;
  }

  /** Each native show: no ring until the first key, no stale question. */
  opened(): void {
    this.focusId = null;
    this.question = null;
    this.error = null;
    this.#pendingNew = null;
    this.#order = this.#view().rows.map((row) => row.id);
  }

  /** Dismissed: a pending question is dropped and the item keeps running. */
  hidden(): void {
    this.question = null;
    this.#pendingNew = null;
  }

  /** Call after every snapshot. */
  sync(): void {
    const rows = this.#view().rows;
    const ids = rows.map((row) => row.id);
    const present = new Set(ids);
    if (this.question !== null && !present.has(this.question.itemId)) this.question = null;
    if (this.focusId !== null && !present.has(this.focusId)) {
      const at = this.#order.indexOf(this.focusId);
      const after = this.#order.slice(at + 1).find((id) => present.has(id));
      const before = this.#order
        .slice(0, Math.max(at, 0))
        .reverse()
        .find((id) => present.has(id));
      this.focusId = at < 0 ? null : (after ?? before ?? null);
    }
    if (this.#pendingNew !== null && present.has(this.#pendingNew)) {
      const id = this.#pendingNew;
      this.#pendingNew = null;
      this.#focus(id);
    }
    this.#order = ids;
  }

  /** Handles one popover key; false when the key is not the popover's (let it through). */
  key(key: PopoverKey): boolean {
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
    switch (key.kind) {
      case "move":
        this.#move(key.to);
        return true;
      case "enter":
        if (this.focusId === null) this.#move("start");
        else void this.select(this.focusId);
        return true;
      case "row": {
        const row = this.#view().rows[key.index];
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
        if (this.focusId === null) this.#move("start");
        else void this.close(this.focusId);
        return true;
      case "escape":
        void this.#effects.hide();
        return true;
    }
  }

  /** Row click, Enter, 1–9: show the item in the main window; the popover closes. */
  async select(itemId: string): Promise<void> {
    if (await this.#send({ type: "select_item", itemId, show: true })) await this.#effects.hide();
  }

  /** j / Jump: the app's waiting order; nothing waiting, nothing happens. */
  async jump(): Promise<void> {
    if (this.#view().waitingCount === 0) return;
    if (await this.#send({ type: "jump_to_waiting" })) await this.#effects.hide();
  }

  /** o / palette button: scoped to the focused row's project. A project's +: that project, in
   *  "new" mode like the sidebar's +. The palette lives in the main window; the popover closes. */
  async palette(project?: { id: string; mode: "new" }): Promise<void> {
    const scope = project?.id ?? this.#focusedRow()?.projectId;
    const command: Extract<PopoverCommand, { type: "open_palette" }> = { type: "open_palette" };
    if (scope !== undefined) command.projectId = scope;
    if (project !== undefined) command.mode = project.mode;
    if (await this.#send(command)) await this.#effects.hide();
  }

  /** h / window button: the main window shows or hides behind; the popover stays open and key. */
  async toggleMain(): Promise<void> {
    try {
      await this.#effects.toggleMain();
    } catch (error) {
      this.error = errorMessage(error);
    }
  }

  /** n/t/e: in the focused row's project, else the launch default. Stays open; the row takes focus. */
  async newItem(kind: NewKind): Promise<void> {
    const projectId = this.#focusedRow()?.projectId;
    const reply = await this.#reply(
      projectId === undefined ? { type: "new_item", kind } : { type: "new_item", projectId, kind },
    );
    if (reply === null || reply.result === "needs-confirmation" || reply.itemId === undefined) return;
    if (this.#view().rows.some((row) => row.id === reply.itemId)) this.#focus(reply.itemId);
    else this.#pendingNew = reply.itemId;
  }

  /** w / ⌫ / row ×: close, or turn the row into the app's question. Stays open. */
  async close(itemId: string, confirmed = false): Promise<void> {
    const before = this.#view().rows.find((row) => row.id === itemId);
    if (before === undefined) return;
    const reply = await this.#reply({ type: "close_item", itemId, confirmed });
    if (reply?.result !== "needs-confirmation") {
      this.question = null;
      return;
    }
    const current = this.#view().rows.find((row) => row.id === itemId) ?? before;
    this.question = { itemId, reason: reply.reason, stamp: closeStamp(current) };
    this.focusId = itemId;
  }

  /** End in the question: confirmed if the item is unchanged, else the app is asked again. */
  async confirm(): Promise<void> {
    const question = this.question;
    if (question === null) return;
    const item = this.#view().rows.find((row) => row.id === question.itemId);
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
    this.#focus(question.itemId);
  }

  /** Edge chip: scroll the row into view and put the ring on it, without selecting it. */
  reveal(itemId: string): void {
    this.question = null;
    this.#focus(itemId);
  }

  /** Keyboard or pointer focus landed on a row (Tab into the list, a click on its ×). */
  focused(itemId: string): void {
    this.focusId = itemId;
  }

  #focusedRow(): ItemState | undefined {
    return this.focusId === null ? undefined : this.#view().rows.find((row) => row.id === this.focusId);
  }

  #focus(itemId: string): void {
    this.focusId = itemId;
    this.#effects.reveal(itemId);
  }

  /** Arrows/Home/End through rows, no wrap. The first key puts the ring on the item the main
   *  window shows, else the first needs-you row, else the first row. */
  #move(to: "up" | "down" | "home" | "end" | "start"): void {
    const { rows, selectedId } = this.#view();
    if (rows.length === 0) return;
    const at = this.focusId === null ? -1 : rows.findIndex((row) => row.id === this.focusId);
    let next: number;
    if (to === "home") next = 0;
    else if (to === "end") next = rows.length - 1;
    else if (at < 0) {
      const selected = rows.findIndex((row) => row.id === selectedId);
      const needs = rows.findIndex((row) => row.activity === "needs-you");
      next = selected >= 0 ? selected : Math.max(needs, 0);
    } else if (to === "up") next = Math.max(0, at - 1);
    else if (to === "down") next = Math.min(rows.length - 1, at + 1);
    else next = at;
    this.#focus(rows[next].id);
  }

  async #send(command: PopoverCommand): Promise<boolean> {
    return (await this.#reply(command)) !== null;
  }

  /** The ok reply, or null after recording the failure. */
  async #reply(command: PopoverCommand): Promise<Extract<PopoverReply, { ok: true }> | null> {
    try {
      const reply = await this.#effects.command(command);
      if (reply.ok) return reply;
      this.error = reply.message;
    } catch (error) {
      this.error = errorMessage(error);
    }
    return null;
  }
}
