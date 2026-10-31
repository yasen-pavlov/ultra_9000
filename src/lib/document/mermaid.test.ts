import { describe, expect, it } from "vitest";
import { MERMAID_SECURE_KEYS, cleanMermaidSvg, mermaidReason, refusedMermaid } from "./mermaid";

const ID = "u9k-mermaid-7";

describe("Mermaid sources that could bring in HTML, links or images are refused before rendering", () => {
  it("refuses HTML labels, links and script", () => {
    expect(refusedMermaid("flowchart TD\n A[\"<img src='https://evil.example/t.png'>\"]")).not.toBeNull();
    expect(refusedMermaid("flowchart TD\n A[\"<a href='/index.html'>go</a>\"]")).toBe(
      "links are not shown in diagrams",
    );
    expect(refusedMermaid('flowchart TD\n A["<foreignObject>x</foreignObject>"]')).not.toBeNull();
    expect(refusedMermaid('flowchart TD\n A --> B\n click A "javascript:alert(1)"')).not.toBeNull();
    expect(refusedMermaid('flowchart TD\n A["<script>alert(1)</script>"]')).not.toBeNull();
  });

  it("refuses image shapes, whose URL Mermaid would preload while rendering", () => {
    expect(refusedMermaid('flowchart TD\n A@{ img: "data:image/png;base64,AAAA" }')).toBe(
      "image shapes are not shown in diagrams",
    );
    expect(refusedMermaid('flowchart TD\n B@{ icon: "fa:user", label: "x" }')).not.toBeNull();
  });

  it("lets ordinary diagrams through", () => {
    expect(refusedMermaid("flowchart LR\n  A[Batch A] --> T\n  B[Batch B] --> T")).toBeNull();
    expect(refusedMermaid("sequenceDiagram\n  Alice->>Bob: a < b")).toBeNull();
  });

  it("locks the label mode and security level against %%{init}%% directives", () => {
    for (const key of ["htmlLabels", "flowchart", "securityLevel", "secure"])
      expect(MERMAID_SECURE_KEYS).toContain(key);
  });
});

describe("Mermaid output is cleaned before insertion", () => {
  const dirty = [
    `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" id="${ID}" onload="alert(1)">`,
    `<style>#${ID}{fill:#fff}</style>`,
    `<g><style>.h-anchor{position:fixed}</style>`,
    `<foreignObject><div class="h-anchor">html</div></foreignObject>`,
    `<image href="data:image/png;base64,AAAA"/>`,
    `<a href="/index.html"><text>link</text></a>`,
    `<script>alert(1)</script>`,
    `<path d="M0 0" marker-end="url(#arrow)" onclick="alert(1)"/>`,
    `<use href="#arrow"/><use xlink:href="https://evil.example/x.svg#a"/>`,
    `<text>label</text></g></svg>`,
  ].join("");

  it("keeps shapes, text, fragment references and Mermaid's own style sheet", () => {
    const svg = cleanMermaidSvg(dirty, ID);
    expect(svg).toContain(`#${ID}{fill:#fff}`);
    expect(svg).toContain("<text>label</text>");
    expect(svg).toContain('href="#arrow"');
  });

  it("drops foreign HTML, images, links, scripts, handlers, outside references and other style sheets", () => {
    const svg = cleanMermaidSvg(dirty, ID);
    for (const gone of [
      "foreignObject",
      "<image",
      "<a ",
      "<script",
      "onload",
      "onclick",
      "evil.example",
      "position:fixed",
      ">link<",
    ]) {
      expect(svg).not.toContain(gone);
    }
  });

  it("refuses output that is not an SVG document", () => {
    expect(cleanMermaidSvg("<div>not svg</div>", ID)).toBe("");
  });
});

describe("mermaidReason", () => {
  it("keeps the first line of Mermaid's error", () => {
    expect(mermaidReason(new Error("Parse error on line 3:\n...A -> T\n---^"))).toBe("parse error on line 3");
    expect(mermaidReason("")).toBe("could not render");
  });
});
