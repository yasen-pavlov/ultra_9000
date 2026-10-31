import { describe, expect, it } from "vitest";
import { clearMatches, firstMatchFrom, markMatches } from "./find";

function page(html: string): HTMLElement {
  const root = document.createElement("article");
  root.innerHTML = html;
  return root;
}

describe("find in document", () => {
  it("marks every case-insensitive match and restores the text when cleared", () => {
    const root = page(
      "<p>Temp dirs: the temp name. <code>TEMP</code></p><h2><button data-reader-chrome>#</button>Temp</h2>",
    );
    const marks = markMatches(root, "temp");
    expect(marks.map((mark) => mark.textContent)).toEqual(["Temp", "temp", "TEMP", "Temp"]);
    expect(root.querySelector("[data-reader-chrome] mark")).toBeNull();
    clearMatches(root);
    expect(root.querySelector("mark")).toBeNull();
    expect(root.textContent).toBe("Temp dirs: the temp name. TEMP#Temp");
  });

  it("re-marks from scratch for a new query", () => {
    const root = page("<p>alpha beta alpha</p>");
    markMatches(root, "alpha");
    expect(markMatches(root, "beta").map((mark) => mark.textContent)).toEqual(["beta"]);
    expect(root.querySelectorAll("mark")).toHaveLength(1);
  });

  it("starts at the first match at or below the reader's place", () => {
    expect(firstMatchFrom([100, 900, 2000], 850)).toBe(1);
    expect(firstMatchFrom([100, 900], 5000)).toBe(0);
  });
});
