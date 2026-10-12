import { describe, expect, it } from "vitest";
import { edgeChipLabel, measureOffscreenNeeds, offscreenRows } from "./edges";

describe("needs-you rows out of view", () => {
  const view = { top: 100, bottom: 300 };
  const row = (id: string, top: number) => ({ id, top, bottom: top + 42 });

  it("splits rows above and below the view in list order", () => {
    const rows = [row("a", 0), row("b", 40), row("c", 150), row("d", 320), row("e", 400)];
    expect(offscreenRows(view, rows)).toEqual({ above: ["a", "b"], below: ["d", "e"] });
  });

  it("counts a row within 4 px of an edge as out of view, a row reaching further in as visible", () => {
    expect(offscreenRows(view, [row("hidden-top", 62), row("hidden-bottom", 296)])).toEqual({
      above: ["hidden-top"],
      below: ["hidden-bottom"],
    });
    expect(offscreenRows(view, [row("peeks-top", 63), row("peeks-bottom", 295)])).toEqual({ above: [], below: [] });
  });

  it("reads the marked rows of a list", () => {
    const list = document.createElement("div");
    list.innerHTML = '<div data-need data-item-id="x"></div><div data-item-id="calm"></div><div data-need></div>';
    const rect = (top: number, bottom: number) => ({ top, bottom }) as DOMRect;
    list.getBoundingClientRect = () => rect(0, 100);
    (list.children[0] as HTMLElement).getBoundingClientRect = () => rect(140, 182);
    expect(measureOffscreenNeeds(list)).toEqual({ above: [], below: ["x"] });
  });

  it("labels the chip with the count and direction", () => {
    expect(edgeChipLabel("down", 1)).toBe("↓ 1 needs you");
    expect(edgeChipLabel("up", 2)).toBe("↑ 2 need you");
  });
});
