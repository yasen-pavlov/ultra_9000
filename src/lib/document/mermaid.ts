// Mermaid diagrams, loaded on first use (never in the initial bundle). Diagrams are SVG and text
// only, styled by the app's own sheet (mermaid.css):
// - before rendering, sources that could reach outside plain shapes and text are refused whole,
//   by construct rather than spelling: extended node metadata `@{…}` (images, icons; Mermaid
//   decodes it as YAML and preloads image URLs while rendering), `click` directives, `%%{…}%%`
//   config, `url(`, any `<`, `href` and `javascript:`;
// - HTML labels are off, strict security is on, and both (with theme and theme CSS) are locked
//   against diagram config;
// - the returned SVG loses every <style>, foreign HTML, image, link, script, handler and outside
//   reference before it reaches the page.
// A refused or failed diagram falls back to its source with a one-line reason (board 32).
import type { Mermaid } from "mermaid";

let loading: Promise<Mermaid> | null = null;
let sequence = 0;

/** Keys a diagram's own config must not change (Mermaid's defaults, label mode, theme and its CSS). */
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
  "theme",
  "themeCSS",
  "themeVariables",
  "fontFamily",
];

function load(): Promise<Mermaid> {
  // The app's diagram styles ship with Mermaid, so neither is in the initial bundle.
  loading ??= Promise.all([import("mermaid"), import("./mermaid.css")]).then(([{ default: mermaid }]) => {
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
        // Inline slice colours: the terminal palette, never orange.
        pie1: "#82aaff",
        pie2: "#c792ea",
        pie3: "#c3e88d",
        pie4: "#89ddff",
        pie5: "#ffcb6b",
        pie6: "#f07178",
        pie7: "#b2ccd6",
        pie8: "#8c8b8b",
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
  [/@\{/, "node metadata (images, icons) is not shown in diagrams"],
  [/^\s*click\s/im, "links and click actions are not shown in diagrams"],
  [/%%\{/, "diagram config is not used in the reader"],
  [/url\(/i, "external resources are not shown in diagrams"],
  // A tag start only: arrows (`<-->`), class stereotypes (`<<interface>>`, the second `<` follows
  // the first) and `x < y` stay ordinary text.
  [/(?<!<)<[A-Za-z/!?]/, "HTML is not shown in diagrams"],
  [/href/i, "links are not shown in diagrams"],
  [/javascript:/i, "links are not shown in diagrams"],
];

/** Why a source is refused before rendering, or null when it may render. */
export function refusedMermaid(source: string): string | null {
  return REFUSED.find(([pattern]) => pattern.test(source))?.[1] ?? null;
}

const DROPPED_ELEMENTS = "style, foreignObject, image, img, a, script, iframe, object, embed";
/** `url(` that is not a reference to a fragment of the same SVG (markers, gradients). */
const OUTSIDE_URL = /url\(\s*(?!['"]?#)/i;

/**
 * Cleans Mermaid's SVG before insertion: no style sheets at all (the app styles diagrams), no
 * foreign HTML, images, links or scripts, no event handlers, and references only to fragments of
 * the same SVG.
 */
export function cleanMermaidSvg(svg: string): string {
  const parsed = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = parsed.documentElement;
  if (root.nodeName.toLowerCase() !== "svg" || parsed.getElementsByTagName("parsererror").length > 0) return "";
  for (const element of [...root.querySelectorAll(DROPPED_ELEMENTS)]) element.remove();
  for (const element of [root, ...root.querySelectorAll("*")]) {
    for (const attribute of [...element.attributes]) {
      const name = attribute.name.toLowerCase();
      const reference = name === "href" || name === "xlink:href" || name === "src";
      if (
        name.startsWith("on") ||
        (reference && !attribute.value.startsWith("#")) ||
        OUTSIDE_URL.test(attribute.value) ||
        /@keyframes|@import/i.test(attribute.value)
      ) {
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
    const clean = cleanMermaidSvg(svg);
    return clean === "" ? { error: "could not render" } : { svg: clean };
  } catch (error) {
    // A failed render can leave its scratch container in the body.
    document.getElementById(`d${id}`)?.remove();
    return { error: mermaidReason(error) };
  }
}
