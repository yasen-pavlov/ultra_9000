import { describe, expect, it, vi } from "vitest";
import { deferred, itemState, settle } from "../test/fixtures";
import type { ItemState, PopoverCommand, PopoverReply } from "../lib/native";
import { PopoverController, type PopoverGroup, type PopoverView } from "./controller.svelte";
import { popoverKey, type PopoverKey } from "./keys";

/** Rows grouped by project, in order of first appearance (the sidebar's order). */
function grouped(rows: readonly ItemState[]): PopoverGroup[] {
  const groups: PopoverGroup[] = [];
  for (const row of rows) {
    const group = groups.find((entry) => entry.projectId === row.projectId);
    if (group === undefined) groups.push({ projectId: row.projectId, items: [row] });
    else group.items = [...group.items, row];
  }
  return groups;
}

type Answer = (command: PopoverCommand) => PopoverReply | Promise<PopoverReply>;

/** A popover over `rows` (or explicit groups) whose app answers every command with `answer`. */
function setup(
  content: ItemState[] | PopoverGroup[],
  options: { selectedId?: string | null; waitingCount?: number } = {},
) {
  const view: PopoverView = {
    groups: content.length > 0 && "items" in content[0] ? (content as PopoverGroup[]) : grouped(content as ItemState[]),
    selectedId: options.selectedId ?? null,
    waitingCount: options.waitingCount ?? 0,
  };
  const calls: string[] = [];
  let answer: Answer = () => ({ ok: true });
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
    calls,
    effects,
    answer: (next: Answer) => (answer = next),
    rows: () => view.groups.flatMap((group) => group.items),
    /** A new snapshot with these rows. */
    snapshot(rows: ItemState[]): void {
      view.groups = grouped(rows);
      controller.sync();
    },
    /** Press a physical key (`repeat`: an OS auto-repeat of a held key) and let replies settle. */
    async press(code: string, repeat = false): Promise<boolean> {
      const key = popoverKey({ code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false });
      const handled = key === null ? false : controller.key(key, repeat);
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
    expect(popover.controller.focus).toBeNull();
    await popover.press("ArrowDown");
    expect(popover.controller.focus).toBe("row:b1");

    popover = setup(rows());
    await popover.press("ArrowUp");
    expect(popover.controller.focus).toBe("row:a2");

    popover = setup([itemState({ id: "x" }), itemState({ id: "y" })]);
    await popover.press("ArrowDown");
    expect(popover.controller.focus).toBe("row:x");
  });

  it("Tab enters the list where the first key would put the ring", () => {
    expect(setup(rows(), { selectedId: "b1" }).controller.entry()).toBe("row:b1");
    expect(setup(rows(), { selectedId: "gone" }).controller.entry()).toBe("row:a2");
    expect(setup([itemState({ id: "x" }), itemState({ id: "y" })]).controller.entry()).toBe("row:x");
    expect(setup([]).controller.entry()).toBeNull();
  });

  it("moves through rows without wrapping; Home and End reach the ends", async () => {
    const popover = setup(rows(), { selectedId: "a1" });
    await popover.press("ArrowUp");
    await popover.press("ArrowUp");
    expect(popover.controller.focus).toBe("row:a1");
    await popover.press("ArrowDown");
    await popover.press("ArrowDown");
    await popover.press("ArrowDown");
    expect(popover.controller.focus).toBe("row:b1");
    await popover.press("Home");
    expect(popover.controller.focus).toBe("row:a1");
    await popover.press("End");
    expect(popover.controller.focus).toBe("row:b1");
    expect(popover.calls).toEqual([]);
  });

  it("Enter or w before any focus only shows the ring", async () => {
    const popover = setup(rows(), { selectedId: "a1" });
    await popover.press("Enter");
    expect(popover.controller.focus).toBe("row:a1");
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
    expect(popover.controller.focus).toBeNull();
  });

  it("hands the ring to the next row when the focused row ends, else the previous", async () => {
    const popover = setup(rows(), { selectedId: "a2" });
    await popover.press("ArrowDown");
    popover.snapshot(popover.rows().filter((row) => row.id !== "a2"));
    expect(popover.controller.focus).toBe("row:b1");
    popover.snapshot(popover.rows().filter((row) => row.id !== "b1"));
    expect(popover.controller.focus).toBe("row:a1");
  });

  it("the edge chip scrolls to the row and rings it without selecting it", () => {
    const popover = setup(rows());
    popover.controller.reveal("a2");
    expect(popover.controller.focus).toBe("row:a2");
    expect(popover.effects.reveal).toHaveBeenCalledWith("row:a2");
    expect(popover.calls).toEqual([]);
  });

  it("arrows keep moving while an arrow key is held", async () => {
    const popover = setup(rows(), { selectedId: "a1" });
    await popover.press("ArrowDown");
    await popover.press("ArrowDown", true);
    await popover.press("ArrowDown", true);
    expect(popover.controller.focus).toBe("row:b1");
  });
});

describe("pinned projects without rows", () => {
  const pinnedOnly = (): PopoverGroup[] => [
    { projectId: atlas, items: [] },
    { projectId: beacon, items: [] },
  ];

  it("each empty project's + is a stop; ↵ on it starts something there", async () => {
    const popover = setup(pinnedOnly());
    expect(popover.controller.entry()).toBe(`plus:${atlas}`);
    await popover.press("ArrowDown");
    expect(popover.controller.focus).toBe(`plus:${atlas}`);
    await popover.press("ArrowDown");
    expect(popover.controller.focus).toBe(`plus:${beacon}`);
    expect(popover.effects.reveal).toHaveBeenLastCalledWith(`plus:${beacon}`);
    await popover.press("Enter");
    expect(popover.calls).toEqual([`{"type":"open_palette","projectId":"${beacon}","mode":"new"}`, "hide"]);
  });

  it("n/t/e start in the focused empty project, w does nothing, the new row takes the ring", async () => {
    const popover = setup(pinnedOnly());
    popover.answer(() => ({ ok: true, itemId: "new" }));
    await popover.press("End");
    await popover.press("KeyW");
    expect(popover.calls).toEqual([]);
    await popover.press("KeyN");
    expect(popover.calls).toEqual([`{"type":"new_item","projectId":"${beacon}","kind":"agent"}`]);
    popover.snapshot([itemState({ id: "new", projectId: beacon })]);
    expect(popover.controller.focus).toBe("row:new");
  });

  it("a project head's + whose project has rows is not a stop", () => {
    const popover = setup(rows());
    popover.controller.focused(`plus:${atlas}`);
    expect(popover.controller.focus).toBeNull();
    popover.controller.focused("row:a1");
    expect(popover.controller.focus).toBe("row:a1");
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
    expect(popover.controller.focus).toBe("row:b1");

    popover.snapshot([...popover.rows(), itemState({ id: "new", projectId: beacon })]);
    expect(popover.controller.focus).toBe("row:new");
    expect(popover.effects.reveal).toHaveBeenLastCalledWith("row:new");
  });

  it("focuses the new row at once when the snapshot arrived before the reply", async () => {
    const popover = setup(rows());
    popover.answer(() => {
      popover.snapshot([...popover.rows(), itemState({ id: "new" })]);
      return { ok: true, itemId: "new" };
    });
    await popover.press("KeyE");
    expect(popover.controller.focus).toBe("row:new");
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

describe("held keys", () => {
  it("holding n/t/e launches once", async () => {
    const popover = setup(rows());
    await popover.press("KeyN");
    await popover.press("KeyN", true);
    await popover.press("KeyN", true);
    expect(popover.calls).toEqual(['{"type":"new_item","kind":"agent"}']);
  });

  it("auto-repeats of ↵, a digit, j, o, h or Esc do nothing", async () => {
    for (const code of ["Enter", "Digit1", "KeyJ", "KeyO", "KeyH", "Escape"]) {
      const popover = setup(rows(), { selectedId: "a1", waitingCount: 1 });
      await popover.press("ArrowDown");
      expect(await popover.press(code, true)).toBe(true);
      expect(popover.calls).toEqual([]);
    }
  });
});

describe("close question", () => {
  function asking(content: ItemState[] = rows(), selectedId = "a1") {
    const popover = setup(content, { selectedId });
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
    expect(popover.controller.question).toMatchObject({ itemId: "a1", reason: "agent-working" });
    await popover.press("KeyW");
    expect(popover.calls).toEqual([
      '{"type":"close_item","itemId":"a1","confirmed":false}',
      '{"type":"close_item","itemId":"a1","confirmed":true}',
    ]);
    expect(popover.controller.question).toBeNull();
  });

  it("holding w, ⌫ or Delete asks once and never confirms", async () => {
    for (const code of ["KeyW", "Backspace", "Delete"]) {
      const popover = asking();
      await popover.press("ArrowDown");
      await popover.press(code);
      await popover.press(code, true);
      await popover.press(code, true);
      expect(popover.calls).toEqual(['{"type":"close_item","itemId":"a1","confirmed":false}']);
      expect(popover.controller.question).not.toBeNull();
    }
  });

  it("a held ↵ does not confirm an open question", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    await popover.press("Enter", true);
    expect(popover.calls).toHaveLength(1);
    expect(popover.controller.question).not.toBeNull();
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
    expect(popover.controller.focus).toBe("row:a1");
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
    expect(popover.controller.focus).toBe("row:a1");
  });

  it("asks again when the item changed state since the question", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    popover.snapshot(
      popover.rows().map((row) => (row.id === "a1" ? { ...row, activity: "needs-you" as const, stateSince: 5 } : row)),
    );
    await popover.press("KeyW");
    expect(popover.calls.at(-1)).toBe('{"type":"close_item","itemId":"a1","confirmed":false}');
  });

  it("asks again when the shell now runs a different job with the same command", async () => {
    const shell = (sinceMs: number) =>
      itemState({ id: "s1", tool: "shell", kind: "terminal", foregroundJob: { command: "cargo", sinceMs } });
    const popover = asking([shell(1)], "s1");
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    popover.snapshot([shell(2)]);
    await popover.press("KeyW");
    expect(popover.calls).toEqual([
      '{"type":"close_item","itemId":"s1","confirmed":false}',
      '{"type":"close_item","itemId":"s1","confirmed":false}',
    ]);

    const unchanged = asking([shell(1)], "s1");
    await unchanged.press("ArrowDown");
    await unchanged.press("KeyW");
    unchanged.snapshot([shell(1)]);
    await unchanged.press("KeyW");
    expect(unchanged.calls.at(-1)).toBe('{"type":"close_item","itemId":"s1","confirmed":true}');
  });

  it("goes away when its item ends elsewhere", async () => {
    const popover = asking();
    await popover.press("ArrowDown");
    await popover.press("KeyW");
    popover.snapshot(popover.rows().filter((row) => row.id !== "a1"));
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

describe("replies that arrive after the popover closed", () => {
  it("a close question answered after hiding or reopening never appears", async () => {
    for (const reopen of [false, true]) {
      const popover = setup(rows(), { selectedId: "a1" });
      const reply = deferred<PopoverReply>();
      popover.answer(() => reply.promise);
      await popover.press("ArrowDown");
      await popover.press("KeyW");
      popover.controller.hidden();
      if (reopen) popover.controller.opened();
      reply.resolve({ ok: true, result: "needs-confirmation", reason: "agent-working" });
      await settle();
      expect(popover.calls).toEqual(['{"type":"close_item","itemId":"a1","confirmed":false}']);
      expect(popover.controller.question).toBeNull();
      expect(popover.controller.focus).toBe(reopen ? null : "row:a1");
    }
  });

  it("a row started in an earlier opening does not take the ring", async () => {
    const popover = setup(rows());
    const reply = deferred<PopoverReply>();
    popover.answer(() => reply.promise);
    await popover.press("KeyN");
    popover.controller.hidden();
    popover.controller.opened();
    reply.resolve({ ok: true, itemId: "new" });
    await settle();
    popover.snapshot([...popover.rows(), itemState({ id: "new" })]);
    expect(popover.controller.focus).toBeNull();
  });

  it("a late select or failure neither hides the new opening nor shows an error in it", async () => {
    const popover = setup(rows());
    const selected = deferred<PopoverReply>();
    popover.answer(() => selected.promise);
    const select = popover.controller.select("a1");
    popover.controller.hidden();
    popover.controller.opened();
    selected.resolve({ ok: true });
    await select;
    expect(popover.calls).toEqual(['{"type":"select_item","itemId":"a1","show":true}']);

    const failed = deferred<PopoverReply>();
    popover.answer(() => failed.promise);
    const launch = popover.controller.newItem("agent");
    popover.controller.opened();
    failed.resolve({ ok: false, error: "launch_failed", message: "No such tool." });
    await launch;
    expect(popover.controller.error).toBeNull();
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
