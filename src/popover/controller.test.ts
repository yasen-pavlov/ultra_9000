import { describe, expect, it, vi } from "vitest";
import { itemState, settle } from "../test/fixtures";
import type { ItemState, PopoverCommand, PopoverReply } from "../lib/native";
import { PopoverController, type PopoverView } from "./controller.svelte";
import { popoverKey, type PopoverKey } from "./keys";

/** A popover over `rows` whose app answers every command with `answer` (ok by default). */
function setup(rows: ItemState[], options: { selectedId?: string | null; waitingCount?: number } = {}) {
  const view: PopoverView = {
    rows,
    selectedId: options.selectedId ?? null,
    waitingCount: options.waitingCount ?? 0,
  };
  const calls: string[] = [];
  let answer: (command: PopoverCommand) => PopoverReply = () => ({ ok: true });
  const effects = {
    command: vi.fn((command: PopoverCommand) => {
      calls.push(JSON.stringify(command));
      return Promise.resolve(answer(command));
    }),
    hide: vi.fn(() => {
      calls.push("hide");
      return Promise.resolve();
    }),
    toggleMain: vi.fn(() => {
      calls.push("toggle-main");
      return Promise.resolve();
    }),
    reveal: vi.fn(),
  };
  const controller = new PopoverController(() => view, effects);
  controller.opened();
  return {
    controller,
    view,
    calls,
    effects,
    answer: (next: (command: PopoverCommand) => PopoverReply) => (answer = next),
    /** Press a physical key and let the replies settle. */
    async press(code: string): Promise<boolean> {
      const key = popoverKey({ code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false });
      const handled = key === null ? false : controller.key(key);
      await settle();
      return handled;
    },
  };
}

const atlas = "/w/atlas";
const beacon = "/w/beacon";
const rows = () => [
  itemState({ id: "a1", projectId: atlas, activity: "working" }),
  itemState({ id: "a2", projectId: atlas, activity: "needs-you", attention: "approval" }),
  itemState({ id: "b1", projectId: beacon, tool: "shell", kind: "terminal" }),
];

describe("focus ring", () => {
  it("starts on the main window's item, else the first needs-you row, else the first row", async () => {
    let popover = setup(rows(), { selectedId: "b1" });
    expect(popover.controller.focusId).toBeNull();
    await popover.press("ArrowDown");
    expect(popover.controller.focusId).toBe("b1");

    popover = setup(rows());
    await popover.press("ArrowUp");
    expect(popover.controller.focusId).toBe("a2");

    popover = setup([itemState({ id: "x" }), itemState({ id: "y" })]);
    await popover.press("ArrowDown");
    expect(popover.controller.focusId).toBe("x");
  });

  it("moves through rows without wrapping; Home and End reach the ends", async () => {
    const popover = setup(rows(), { selectedId: "a1" });
    await popover.press("ArrowUp");
    await popover.press("ArrowUp");
    expect(popover.controller.focusId).toBe("a1");
    await popover.press("ArrowDown");
    await popover.press("ArrowDown");
    await popover.press("ArrowDown");
    expect(popover.controller.focusId).toBe("b1");
    await popover.press("Home");
    expect(popover.controller.focusId).toBe("a1");
    await popover.press("End");
    expect(popover.controller.focusId).toBe("b1");
    expect(popover.calls).toEqual([]);
  });

  it("Enter or w before any focus only shows the ring", async () => {
    const popover = setup(rows(), { selectedId: "a1" });
    await popover.press("Enter");
    expect(popover.controller.focusId).toBe("a1");
    expect(popover.calls).toEqual([]);
  });

  it("each opening hides the ring and drops a pending question", async () => {
    const popover = setup(rows(), { selectedId: "a1" });
    popover.answer(() => ({ ok: true, result: "needs-confirmation", reason: "agent-working" }));
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    expect(popover.controller.question).not.toBeNull();
    popover.controller.hidden();
    expect(popover.controller.question).toBeNull();
    popover.controller.opened();
    expect(popover.controller.focusId).toBeNull();
  });

  it("hands the ring to the next row when the focused row ends, else the previous", async () => {
    const popover = setup(rows(), { selectedId: "a2" });
    await popover.press("ArrowDown");
    popover.view.rows = popover.view.rows.filter((row) => row.id !== "a2");
    popover.controller.sync();
    expect(popover.controller.focusId).toBe("b1");
    popover.view.rows = popover.view.rows.filter((row) => row.id !== "b1");
    popover.controller.sync();
    expect(popover.controller.focusId).toBe("a1");
  });

  it("the edge chip scrolls to the row and rings it without selecting it", () => {
    const popover = setup(rows());
    popover.controller.reveal("a2");
    expect(popover.controller.focusId).toBe("a2");
    expect(popover.effects.reveal).toHaveBeenCalledWith("a2");
    expect(popover.calls).toEqual([]);
  });
});

describe("keys that close the popover", () => {
  it("Enter shows the focused item in the main window, then hides", async () => {
    const popover = setup(rows(), { selectedId: "a1" });
    await popover.press("ArrowDown");
    await popover.press("ArrowDown");
    await popover.press("Enter");
    expect(popover.calls).toEqual(['{"type":"select_item","itemId":"a2","show":true}', "hide"]);
  });

  it("1–9 pick rows in list order; a digit past the last row does nothing", async () => {
    const many = Array.from({ length: 10 }, (_, index) => itemState({ id: `r${index + 1}` }));
    let popover = setup(many);
    await popover.press("Digit1");
    expect(popover.calls).toEqual(['{"type":"select_item","itemId":"r1","show":true}', "hide"]);

    popover = setup(many);
    await popover.press("Digit9");
    expect(popover.calls[0]).toBe('{"type":"select_item","itemId":"r9","show":true}');

    popover = setup(many);
    expect(await popover.press("Digit0")).toBe(false);
    expect(popover.calls).toEqual([]);

    popover = setup(rows());
    await popover.press("Digit4");
    expect(popover.calls).toEqual([]);
  });

  it("j jumps and hides; with nothing waiting it does nothing", async () => {
    let popover = setup(rows(), { waitingCount: 1 });
    await popover.press("KeyJ");
    expect(popover.calls).toEqual(['{"type":"jump_to_waiting"}', "hide"]);

    popover = setup(rows(), { waitingCount: 0 });
    await popover.press("KeyJ");
    expect(popover.calls).toEqual([]);
  });

  it("o opens the palette in the focused row's project, then hides", async () => {
    let popover = setup(rows(), { selectedId: "b1" });
    await popover.press("ArrowDown");
    await popover.press("KeyO");
    expect(popover.calls).toEqual([`{"type":"open_palette","projectId":"${beacon}"}`, "hide"]);

    popover = setup(rows(), { selectedId: "b1" });
    await popover.press("KeyO");
    expect(popover.calls).toEqual(['{"type":"open_palette"}', "hide"]);
  });

  it("a project's + opens the palette to start something there", async () => {
    const popover = setup(rows());
    await popover.controller.palette({ id: atlas, mode: "new" });
    expect(popover.calls).toEqual([`{"type":"open_palette","projectId":"${atlas}","mode":"new"}`, "hide"]);
  });

  it("Esc hides", async () => {
    const popover = setup(rows());
    await popover.press("Escape");
    expect(popover.calls).toEqual(["hide"]);
  });

  it("a failed command keeps the popover open and shows why", async () => {
    const popover = setup(rows());
    popover.answer(() => ({ ok: false, error: "unknown_item", message: "Item r1 does not exist." }));
    await popover.controller.select("a1");
    expect(popover.calls).not.toContain("hide");
    expect(popover.controller.error).toBe("Item r1 does not exist.");
    await popover.press("ArrowDown");
    expect(popover.controller.error).toBeNull();
  });
});

describe("keys that keep the popover open", () => {
  it("h shows or hides the main window and stays open", async () => {
    const popover = setup(rows());
    await popover.press("KeyH");
    expect(popover.calls).toEqual(["toggle-main"]);
  });

  it("n/t/e start in the focused row's project and focus the new row once it appears", async () => {
    const popover = setup(rows(), { selectedId: "b1" });
    popover.answer(() => ({ ok: true, itemId: "new" }));
    await popover.press("ArrowDown");
    await popover.press("KeyT");
    expect(popover.calls).toEqual([`{"type":"new_item","projectId":"${beacon}","kind":"shell"}`]);
    expect(popover.controller.focusId).toBe("b1");

    popover.view.rows = [...popover.view.rows, itemState({ id: "new", projectId: beacon })];
    popover.controller.sync();
    expect(popover.controller.focusId).toBe("new");
    expect(popover.effects.reveal).toHaveBeenLastCalledWith("new");
  });

  it("focuses the new row at once when the snapshot arrived before the reply", async () => {
    const popover = setup(rows());
    popover.answer(() => {
      popover.view.rows = [...popover.view.rows, itemState({ id: "new" })];
      return { ok: true, itemId: "new" };
    });
    await popover.press("KeyE");
    expect(popover.controller.focusId).toBe("new");
  });

  it("without focus or rows, n/t/e use the app's launch default", async () => {
    let popover = setup(rows(), { selectedId: "a1" });
    await popover.press("KeyN");
    expect(popover.calls).toEqual(['{"type":"new_item","kind":"agent"}']);

    popover = setup([]);
    await popover.press("KeyE");
    expect(popover.calls).toEqual(['{"type":"new_item","kind":"neovim"}']);
  });

  it("w closes the focused item when the app needs no confirmation", async () => {
    const popover = setup(rows(), { selectedId: "b1" });
    popover.answer(() => ({ ok: true, result: "closed" }));
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    expect(popover.calls).toEqual(['{"type":"close_item","itemId":"b1","confirmed":false}']);
    expect(popover.controller.question).toBeNull();
  });
});

describe("close question", () => {
  function asking() {
    const popover = setup(rows(), { selectedId: "a1" });
    popover.answer((command) =>
      command.type === "close_item" && command.confirmed !== true
        ? { ok: true, result: "needs-confirmation", reason: "agent-working" }
        : { ok: true, result: "closed" },
    );
    return popover;
  }

  it("w asks, the same key confirms", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    expect(popover.controller.question).toEqual({ itemId: "a1", reason: "agent-working", stamp: "working|0|" });
    await popover.press("KeyW");
    expect(popover.calls).toEqual([
      '{"type":"close_item","itemId":"a1","confirmed":false}',
      '{"type":"close_item","itemId":"a1","confirmed":true}',
    ]);
    expect(popover.controller.question).toBeNull();
  });

  it("Delete and Backspace are close too; Enter confirms an open question", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("Backspace");
    expect(popover.controller.question).not.toBeNull();
    await popover.press("Enter");
    expect(popover.calls.at(-1)).toBe('{"type":"close_item","itemId":"a1","confirmed":true}');
    expect(await popover.press("Delete")).toBe(true);
  });

  it("Esc cancels the question first and keeps the item; a second Esc hides", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    await popover.press("Escape");
    expect(popover.controller.question).toBeNull();
    expect(popover.controller.focusId).toBe("a1");
    expect(popover.calls).toEqual(['{"type":"close_item","itemId":"a1","confirmed":false}']);
    await popover.press("Escape");
    expect(popover.calls.at(-1)).toBe("hide");
  });

  it("arrows back out without moving", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    await popover.press("ArrowDown");
    expect(popover.controller.question).toBeNull();
    expect(popover.controller.focusId).toBe("a1");
  });

  it("asks again when the item changed state since the question", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    popover.view.rows = popover.view.rows.map((row) =>
      row.id === "a1" ? { ...row, activity: "needs-you" as const, stateSince: 5 } : row,
    );
    popover.controller.sync();
    await popover.press("KeyW");
    expect(popover.calls.at(-1)).toBe('{"type":"close_item","itemId":"a1","confirmed":false}');
    expect(popover.controller.question?.stamp).toBe("needs-you|5|");
  });

  it("goes away when its item ends elsewhere", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    popover.view.rows = popover.view.rows.filter((row) => row.id !== "a1");
    popover.controller.sync();
    expect(popover.controller.question).toBeNull();
  });

  it("other keys still act while it is open", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    await popover.press("KeyH");
    expect(popover.calls.at(-1)).toBe("toggle-main");
    expect(popover.controller.question).not.toBeNull();
  });
});

describe("keys the popover leaves alone", () => {
  it("modified keys and unmapped keys pass through", () => {
    const press = (code: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}) =>
      popoverKey({ code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods });
    expect(press("KeyQ", { metaKey: true })).toBeNull();
    expect(press("KeyW", { ctrlKey: true })).toBeNull();
    expect(press("KeyJ", { shiftKey: true })).toBeNull();
    expect(press("Tab")).toBeNull();
    expect(press("KeyX")).toBeNull();
    const numpad: PopoverKey | null = press("Numpad3");
    expect(numpad).toEqual({ kind: "row", index: 2 });
  });
});
