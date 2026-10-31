import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { MERMAID_SECURE_KEYS, cleanMermaidSvg, mermaidReason, refusedMermaid, renderMermaid } from "./mermaid";
// Vitest stubs style sheets (even ?raw), so the test reads the shipped file itself; tests run
// from the repository root.
import { readFileSync } from "node:fs";

describe("Mermaid sources that reach beyond shapes and text are refused before rendering", () => {
  it("refuses extended node metadata whatever its spelling (Mermaid decodes it as YAML and preloads images)", () => {
    for (const metadata of [
      'A@{ img: "https://evil.example/t.png" }',
      'A@{ "img": "https://evil.example/t.png" }',
      'A@{ "\\u0069mg": "data:image/png;base64,AAAA" }',
      'B@{ icon: "fa:user", label: "x" }',
      "C@{ shape: rect }",
    ]) {
      expect(refusedMermaid(`flowchart TD\n ${metadata}`)).toBe(
        "node metadata (images, icons) is not shown in diagrams",
      );
    }
  });

  it("refuses click directives, including a real flowchart link", () => {
    expect(refusedMermaid('flowchart TD\n A[Important]-->B[Result]\n click A href "https://example.com"')).toBe(
      "links and click actions are not shown in diagrams",
    );
    expect(refusedMermaid("flowchart TD\n A-->B\n  click A callback")).not.toBeNull();
  });

  it("refuses config directives, CSS URLs, any markup, href and javascript:", () => {
    expect(
      refusedMermaid(
        '%%{init: {"themeCSS":"@keyframes u9kspin{} .node{fill:url(https://evil.example/x.svg)}"}}%%\nflowchart TD\n A-->B',
      ),
    ).toBe("diagram config is not used in the reader");
    expect(refusedMermaid("flowchart TD\n A[url(x)]-->B")).not.toBeNull();
    expect(refusedMermaid('flowchart TD\n A["<b>bold</b>"]')).toBe("HTML is not shown in diagrams");
    expect(refusedMermaid("flowchart TD\n A[see href]")).not.toBeNull();
    expect(refusedMermaid("flowchart TD\n A[javascript:alert(1)]")).not.toBeNull();
  });

  it("refuses only HTML tag starts, not arrows, stereotypes or comparisons", () => {
    for (const tag of [
      'A["<img src=x>"]',
      'A["<a href=x>go</a>"]',
      'A["x</span>"]',
      "A[<!-- note -->]",
      'A["<?xml version=1?>"]',
    ]) {
      expect(refusedMermaid(`flowchart TD\n ${tag}`)).not.toBeNull();
    }
    expect(refusedMermaid("flowchart LR\n A <--> B")).toBeNull();
    expect(refusedMermaid("classDiagram\n class A {\n  <<interface>>\n }")).toBeNull();
    expect(refusedMermaid('flowchart TD\n A["x < y"] --> B')).toBeNull();
  });

  it("lets ordinary diagrams through", () => {
    expect(refusedMermaid("flowchart LR\n  A[Batch A] --> T\n  B[Batch B] --> T")).toBeNull();
    expect(refusedMermaid("sequenceDiagram\n  Alice->>Bob: hello\n  Bob-->>Alice: done")).toBeNull();
  });

  it("locks label mode, security, theme and theme CSS against diagram config", () => {
    for (const key of ["htmlLabels", "flowchart", "securityLevel", "secure", "theme", "themeCSS"]) {
      expect(MERMAID_SECURE_KEYS).toContain(key);
    }
  });
});

describe("Mermaid output is cleaned before insertion", () => {
  const dirty = [
    '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" id="d1" onload="alert(1)">',
    "<style>#d1{fill:#fff} @keyframes u9kspin{} .node{fill:url(https://evil.example/x.svg)}</style>",
    "<g><style>.h-anchor{position:fixed}</style>",
    '<foreignObject><div class="h-anchor">html</div></foreignObject>',
    '<image href="data:image/png;base64,AAAA"/>',
    '<a href="/index.html"><text>link</text></a>',
    "<script>alert(1)</script>",
    '<path d="M0 0" marker-end="url(#arrow)" onclick="alert(1)"/>',
    '<rect style="fill:url(https://evil.example/y.svg)" width="4"/>',
    '<use href="#arrow"/><use xlink:href="https://evil.example/x.svg#a"/>',
    "<text>label</text></g></svg>",
  ].join("");

  it("keeps shapes, text and references to fragments of the same SVG", () => {
    const svg = cleanMermaidSvg(dirty);
    expect(svg).toContain("<text>label</text>");
    expect(svg).toContain('href="#arrow"');
    expect(svg).toContain('marker-end="url(#arrow)"');
  });

  it("drops every style sheet, foreign HTML, images, links, scripts, handlers and outside URLs", () => {
    const svg = cleanMermaidSvg(dirty);
    expect(svg).not.toContain("<style");
    expect(svg).not.toContain("@keyframes");
    expect(svg).not.toMatch(/url\((?!#)/);
    for (const gone of ["foreignObject", "<image", "<a ", "<script", "onload", "onclick", "evil.example", ">link<"]) {
      expect(svg).not.toContain(gone);
    }
  });

  it("refuses output that is not an SVG document", () => {
    expect(cleanMermaidSvg("<div>not svg</div>")).toBe("");
  });
});

describe("renderMermaid on a real diagram", () => {
  // jsdom has no SVG layout; Mermaid only needs plausible text boxes. Plain prototype properties:
  // each test file runs in its own environment.
  beforeEach(() => {
    const box = { x: 0, y: 0, width: 80, height: 20, top: 0, left: 0, right: 80, bottom: 20 };
    Object.assign(window.SVGElement.prototype, {
      getBBox: () => box,
      getComputedTextLength: () => 80,
    });
  });
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("keeps every label, with no style sheet and no outside URL", async () => {
    const result = await renderMermaid("flowchart TD\n A[Important]-->B[Result]");
    expect("svg" in result).toBe(true);
    const svg = "svg" in result ? result.svg : "";
    expect(svg).toContain("Important");
    expect(svg).toContain("Result");
    expect(svg).not.toContain("<style");
    expect(svg).not.toContain("@keyframes");
    expect(svg).not.toMatch(/url\((?!#)/);
    expect(svg).not.toContain("foreignObject");
  });

  it("centres flowchart and state node labels only, and colours class relation ends, with the app's sheet", async () => {
    const flow = await renderMermaid("flowchart TD\n A[Important]-->B[Result]");
    const state = await renderMermaid("stateDiagram-v2\n Idle --> Busy");
    const classes = await renderMermaid(
      "classDiagram\n class Store {\n  +load()\n  -cache\n }\n Animal <|-- Duck\n Car *-- Wheel",
    );
    // jsdom has no SVG presentation cascade (nor these SVG properties): read the sheet's rules
    // from its text and check which of them match.
    const diagramCss = readFileSync("src/lib/document/mermaid.css", "utf8");
    const rules = [...diagramCss.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)].map((match) => ({
      selector: match[1].trim(),
      body: match[2],
    }));
    const ruled = (element: Element, property: string, value: string): boolean =>
      rules.some((rule) => rule.body.includes(`${property}: ${value};`) && element.matches(rule.selector));
    const article = document.createElement("article");
    article.className = "prose";
    article.innerHTML = [flow, state, classes]
      .map((result) => `<div class="diagram">${"svg" in result ? result.svg : ""}</div>`)
      .join("");
    document.body.append(article);
    const [flowBox, stateBox, classBox] = [...article.querySelectorAll(".diagram")];
    const flowLabels = [...flowBox.querySelectorAll(".node .label text")];
    expect(flowLabels.map((text) => text.textContent)).toEqual(["Important", "Result"]);
    const stateLabels = [...stateBox.querySelectorAll(".node .label text")];
    expect(stateLabels.map((text) => text.textContent)).toEqual(expect.arrayContaining(["Idle", "Busy"]));
    for (const text of [...flowLabels, ...stateLabels]) expect(ruled(text, "text-anchor", "middle")).toBe(true);
    // Class titles and members are placed for a start anchor by Mermaid: never re-anchored.
    const classTexts = [...classBox.querySelectorAll("text")];
    expect(classTexts.map((text) => text.textContent)).toEqual(expect.arrayContaining(["Store", "+load()", "-cache"]));
    for (const text of classTexts) expect(ruled(text, "text-anchor", "middle")).toBe(false);
    const markers = [...classBox.querySelectorAll("marker.marker")];
    expect(markers.length).toBeGreaterThan(0);
    for (const marker of markers) expect(ruled(marker, "fill", "var(--dim)")).toBe(true);
    article.remove();
  });

  it("renders bidirectional arrows and class stereotypes", async () => {
    const arrows = await renderMermaid("flowchart LR\n A[Left] <--> B[Right]");
    expect("svg" in arrows && arrows.svg.includes("Left") && arrows.svg.includes("Right")).toBe(true);
    const classes = await renderMermaid("classDiagram\n class Store {\n  <<interface>>\n  +load()\n }");
    expect("svg" in classes && classes.svg.includes("Store")).toBe(true);
  });

  it("falls back to the source for a link-bearing flowchart without rendering it", async () => {
    expect(await renderMermaid('flowchart TD\n A[Important]-->B[Result]\n click A href "https://example.com"')).toEqual(
      {
        error: "links and click actions are not shown in diagrams",
      },
    );
  });
});

describe("mermaidReason", () => {
  it("keeps the first line of Mermaid's error", () => {
    expect(mermaidReason(new Error("Parse error on line 3:\n...A -> T\n---^"))).toBe("parse error on line 3");
    expect(mermaidReason("")).toBe("could not render");
  });
});
