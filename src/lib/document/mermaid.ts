// Mermaid diagrams, loaded on first use (never in the initial bundle). Diagrams are SVG and text
// only: HTML labels are off and locked against `%%{init}%%` directives, sources that would bring in
// HTML, links or images are refused before Mermaid sees them (its image shapes preload their URL
// while rendering, so cleaning the output alone is too late), and the returned SVG is cleaned
// again before it reaches the page. A diagram that fails or is refused falls back to its source
// with a one-line reason (board 32).
import type { Mermaid } from "mermaid";

let loading: Promise<Mermaid> | null = null;
let sequence = 0;

/** Keys a diagram's own `%%{init}%%` must not change (Mermaid's defaults plus the label mode). */
export const MERMAID_SECURE_KEYS = [
  "secure",
  "securityLevel",
  "startOnLoad",
  "maxTextSize",
  "suppressErrorRendering",
  "maxEdges",
  "htmlLabels",
  "flowchart",
  "class",
  "state",
  "er",
  "requirement",
  "mindmap",
  "block",
];

function load(): Promise<Mermaid> {
  loading ??= import("mermaid").then(({ default: mermaid }) => {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      secure: MERMAID_SECURE_KEYS,
      htmlLabels: false,
      flowchart: { htmlLabels: false },
      class: { htmlLabels: false },
      theme: "base",
      fontFamily: "var(--ui)",
      themeVariables: {
        darkMode: true,
        background: "#212121",
        primaryColor: "#2b2b2b",
        primaryBorderColor: "#474747",
        primaryTextColor: "#e4ebee",
        secondaryColor: "#272727",
        tertiaryColor: "#1c1c1c",
        lineColor: "#8c8b8b",
        textColor: "#b0bec5",
        mainBkg: "#2b2b2b",
        nodeBorder: "#474747",
        clusterBkg: "#1c1c1c",
        edgeLabelBackground: "#212121",
      },
    });
    return mermaid;
  });
  loading.catch(() => {
    loading = null;
  });
  return loading;
}

const REFUSED: readonly [RegExp, string][] = [
  [/<\s*(img|image|foreignobject|script)\b/i, "HTML and images are not shown in diagrams"],
  [/<\s*a[\s>]/i, "links are not shown in diagrams"],
  [/javascript\s*:/i, "links are not shown in diagrams"],
  [/@\{[^}]*\b(img|image|icon)\s*:/is, "image shapes are not shown in diagrams"],
];

/** Why a source is refused before rendering, or null when it may render. */
export function refusedMermaid(source: string): string | null {
  return REFUSED.find(([pattern]) => pattern.test(source))?.[1] ?? null;
}

const DROPPED_ELEMENTS = "foreignObject, image, img, a, script, iframe, object, embed";

/**
 * Cleans Mermaid's SVG before insertion: no foreign HTML, images, links or scripts, no event
 * handlers, references only to fragments in the same SVG, and only Mermaid's own top-level style
 * sheet (scoped to the diagram id).
 */
export function cleanMermaidSvg(svg: string, id: string): string {
  const parsed = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = parsed.documentElement;
  if (root.nodeName.toLowerCase() !== "svg" || parsed.getElementsByTagName("parsererror").length > 0) return "";
  for (const element of [...root.querySelectorAll(DROPPED_ELEMENTS)]) element.remove();
  for (const style of [...root.querySelectorAll("style")]) {
    const own = style.parentElement === root && (style.textContent ?? "").trimStart().startsWith(`#${id}`);
    if (!own) style.remove();
  }
  for (const element of [root, ...root.querySelectorAll("*")]) {
    for (const attribute of [...element.attributes]) {
      const name = attribute.name.toLowerCase();
      const reference = name === "href" || name === "xlink:href" || name === "src";
      if (name.startsWith("on") || (reference && !attribute.value.startsWith("#"))) {
        element.removeAttributeNode(attribute);
      }
    }
  }
  return new XMLSerializer().serializeToString(root);
}

/** The first line of Mermaid's error: “Parse error on line 3:” → “parse error on line 3”. */
export function mermaidReason(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  const first = message.split("\n")[0].replace(/:$/, "").trim();
  if (first === "") return "could not render";
  return first[0].toLowerCase() + first.slice(1);
}

export async function renderMermaid(source: string): Promise<{ svg: string } | { error: string }> {
  const refused = refusedMermaid(source);
  if (refused !== null) return { error: refused };
  sequence += 1;
  const id = `u9k-mermaid-${sequence}`;
  try {
    const mermaid = await load();
    const { svg } = await mermaid.render(id, source);
    const clean = cleanMermaidSvg(svg, id);
    return clean === "" ? { error: "could not render" } : { svg: clean };
  } catch (error) {
    // A failed render can leave its scratch container in the body.
    document.getElementById(`d${id}`)?.remove();
    return { error: mermaidReason(error) };
  }
}
