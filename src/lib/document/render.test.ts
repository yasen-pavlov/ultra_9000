import { describe, expect, it, vi } from "vitest";
import { loadHighlighter } from "./highlight";
import { renderMarkdown, type RenderOptions } from "./render";

const PIXEL = "data:image/png;base64,iVBORw0KGgo=";

function options(overrides: Partial<RenderOptions> = {}): RenderOptions {
  return { loadAsset: () => Promise.resolve(null), highlighter: null, ...overrides };
}

function dom(html: string): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = html;
  return root;
}

describe("renderMarkdown sanitisation", () => {
  it("drops raw HTML: scripts, iframes, event handlers and inline markup never reach the page", async () => {
    const { html } = await renderMarkdown(
      [
        "# Title <b>bold</b>",
        "",
        "<script>alert(1)</script>",
        "",
        '<iframe src="https://evil.example"></iframe>',
        "",
        'Text <img src=x onerror="alert(1)"> and <a href="https://x" onclick="alert(1)">a</a>.',
      ].join("\n"),
      options(),
    );
    const root = dom(html);
    expect(root.querySelector("script, iframe, b")).toBeNull();
    expect(html).not.toMatch(/onerror|onclick|<img/);
    expect(root.querySelector("h1")?.textContent).toBe("Title bold");
  });

  it("drops javascript: and other unsafe link targets but keeps https links", async () => {
    const { html } = await renderMarkdown("[bad](javascript:alert(1)) [good](https://example.com/a)", options());
    const links = [...dom(html).querySelectorAll("a")];
    expect(links.map((link) => link.getAttribute("href"))).toEqual([null, "https://example.com/a"]);
  });

  it("loads remote images over https only, with no referrer; http: images become their alt text and URL", async () => {
    const { html } = await renderMarkdown(
      "![chart](https://cdn.example/c.png) ![old](http://cdn.example/o.png)",
      options(),
    );
    const root = dom(html);
    const images = root.querySelectorAll("img");
    expect(images).toHaveLength(1);
    expect(images[0].getAttribute("src")).toBe("https://cdn.example/c.png");
    expect(images[0].getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(root.querySelector(".img-fallback")?.textContent).toBe("old · http://cdn.example/o.png");
  });

  it("serves relative images as data: URLs from document_asset, once per path, and drops authored data: URLs", async () => {
    const loadAsset = vi.fn((path: string) => Promise.resolve(path === "img/fig 1.png" ? PIXEL : null));
    const { html } = await renderMarkdown(
      "![a](img/fig%201.png) ![b](img/fig%201.png) ![c](../escape.png) ![d](data:image/png;base64,AAAA)",
      options({ loadAsset }),
    );
    const root = dom(html);
    expect([...root.querySelectorAll("img")].map((image) => image.getAttribute("src"))).toEqual([PIXEL, PIXEL]);
    expect(loadAsset).toHaveBeenCalledTimes(2);
    expect(loadAsset).toHaveBeenCalledWith("img/fig 1.png");
    expect(loadAsset).toHaveBeenCalledWith("../escape.png");
    expect([...root.querySelectorAll(".img-fallback")].map((node) => node.textContent)).toEqual([
      "c · ../escape.png",
      "d",
    ]);
  });
});

describe("renderMarkdown structure", () => {
  it("slugs headings uniquely, prefixes their ids and records their source lines", async () => {
    const result = await renderMarkdown("# Report\n\n## Summary\n\ntext\n\n## Summary\n\n### Root cause", options());
    expect(result.h1).toBe("Report");
    expect(result.headings.map(({ level, slug, line }) => [level, slug, line])).toEqual([
      [1, "report", 1],
      [2, "summary", 3],
      [2, "summary-1", 7],
      [3, "root-cause", 9],
    ]);
    const root = dom(result.html);
    expect(root.querySelector("#user-content-summary-1")?.getAttribute("data-line")).toBe("7");
    expect(root.querySelector("p")?.getAttribute("data-line")).toBe("5");
  });

  it("turns GitHub callouts into labelled quotes without the marker", async () => {
    const { html } = await renderMarkdown("> [!WARNING]\n> Old runs may leave directories.", options());
    const quote = dom(html).querySelector("blockquote");
    expect(quote?.className).toBe("callout callout-warning");
    expect(quote?.querySelector(".callout-label")?.textContent).toBe("Warning");
    expect(quote?.textContent).not.toContain("[!WARNING]");
    expect(quote?.textContent).toContain("Old runs may leave directories.");
  });

  it("leaves ordinary quotes alone", async () => {
    const { html } = await renderMarkdown("> [!UNKNOWN] text", options());
    expect(dom(html).querySelector("blockquote")?.className).toBe("");
  });

  it("takes frontmatter out of the prose into a collapsed header", async () => {
    const result = await renderMarkdown("---\ntitle: Importer rollover\nstatus: draft\n---\n# Body", options());
    expect(result.frontmatter?.fields).toEqual([
      { key: "title", value: "Importer rollover" },
      { key: "status", value: "draft" },
    ]);
    expect(result.html).not.toContain("status");
    expect(result.headings[0].line).toBe(5);
  });

  it("keeps footnotes linked and renders task lists disabled", async () => {
    const { html } = await renderMarkdown("Claim[^1]\n\n- [x] done\n- [ ] open\n\n[^1]: Source.", options());
    const root = dom(html);
    const ref = root.querySelector("a[data-footnote-ref]");
    const target = ref?.getAttribute("href")?.slice(1) ?? "";
    expect(root.querySelector(`[id="user-content-${target}"]`)?.textContent).toContain("Source.");
    const boxes = [...root.querySelectorAll<HTMLInputElement>("input[type=checkbox]")];
    expect(boxes.map((box) => [box.checked, box.disabled])).toEqual([
      [true, true],
      [false, true],
    ]);
  });

  it("wraps code blocks with their language and marks Mermaid sources with their line count", async () => {
    const result = await renderMarkdown(
      "```rust\nfn a() {}\n```\n\n```mermaid\nflowchart LR\n  A --> B\n```",
      options(),
    );
    const root = dom(result.html);
    expect(root.querySelector(".code-block")?.getAttribute("data-lang")).toBe("rust");
    expect(root.querySelector(".mermaid")?.getAttribute("data-lines")).toBe("2");
    expect(root.querySelector(".mermaid code")?.textContent).toContain("A --> B");
    expect(result.needsHighlighter).toBe(true);
  });

  it("highlights known languages with the app theme once Shiki is loaded, and only those", async () => {
    const highlighter = await loadHighlighter();
    const result = await renderMarkdown(
      "```rust\nfn main() {}\n```\n\n```klingon\nqapla\n```",
      options({ highlighter }),
    );
    const root = dom(result.html);
    const shiki = root.querySelector(".code-block pre.shiki");
    expect(shiki?.textContent).toBe("fn main() {}");
    expect(shiki?.querySelector("span[style]")).not.toBeNull();
    expect(root.querySelectorAll(".code-block pre.shiki")).toHaveLength(1);
    expect(result.needsHighlighter).toBe(false);
  });

  it("renders math with KaTeX", async () => {
    const { html } = await renderMarkdown("Inline $x^2$ and\n\n$$\n\\frac{a}{b}\n$$", options());
    const root = dom(html);
    expect(root.querySelectorAll(".katex").length).toBeGreaterThanOrEqual(2);
    expect(root.querySelector("annotation")?.textContent).toBe("x^2");
  });

  it("marks numeric table cells", async () => {
    const { html } = await renderMarkdown("| Runs | Rate |\n| - | -: |\n| ci | 4.0 % |", options());
    expect([...dom(html).querySelectorAll("td")].map((cell) => cell.className)).toEqual(["", "num"]);
  });
});
