// The sanitize schema for rendered documents (contract item 7). GitHub's default schema, extended
// only for what the app's own plugins emit: KaTeX's spans, MathML and stretchy SVG; Shiki's token
// spans; the reader's structural classes (callouts, code blocks, Mermaid sources, frontmatter) and
// `data-line`. Raw HTML never reaches it (remark-rehype drops it), so markdown authors cannot
// produce any of these extensions themselves except through the syntax that maps to them.
import { defaultSchema, type Options as Schema } from "rehype-sanitize";

type Attributes = NonNullable<Schema["attributes"]>;
type AttributeDefinition = Attributes[string][number];

const CALLOUT_KINDS = ["note", "tip", "warning", "important", "caution"] as const;
export type CalloutKind = (typeof CALLOUT_KINDS)[number];

/** Elements KaTeX's MathML output uses (output: "htmlAndMathml"; the MathML half is for screen readers). */
const MATHML = [
  "math",
  "semantics",
  "annotation",
  "mrow",
  "mi",
  "mn",
  "mo",
  "ms",
  "mspace",
  "mtext",
  "msup",
  "msub",
  "msubsup",
  "mfrac",
  "msqrt",
  "mroot",
  "mtable",
  "mtr",
  "mtd",
  "munder",
  "mover",
  "munderover",
  "mpadded",
  "mphantom",
  "menclose",
  "mstyle",
];

const MATHML_ATTRIBUTES: AttributeDefinition[] = [
  "xmlns",
  "display",
  "encoding",
  "mathvariant",
  "stretchy",
  "fence",
  "separator",
  "lspace",
  "rspace",
  "accent",
  "accentunder",
  "columnalign",
  "rowspacing",
  "columnspacing",
  "width",
  "height",
  "depth",
  "minsize",
  "maxsize",
  "movablelimits",
  "scriptlevel",
  "displaystyle",
  "linethickness",
  "notation",
];

const BLOCKS_WITH_LINE = [
  "p",
  "ul",
  "ol",
  "li",
  "blockquote",
  "table",
  "hr",
  "div",
  "pre",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
];

function extend(name: string, ...extra: AttributeDefinition[]): [string, AttributeDefinition[]] {
  return [name, [...(defaultSchema.attributes?.[name] ?? []), ...extra]];
}

const attributes: Attributes = { ...defaultSchema.attributes };
for (const tag of BLOCKS_WITH_LINE) attributes[tag] = [...(attributes[tag] ?? []), "dataLine"];
Object.assign(
  attributes,
  Object.fromEntries([
    // KaTeX and Shiki: arbitrary class names and inline geometry/colour on spans (only those
    // plugins emit spans), stretchy delimiters as SVG.
    extend("span", "className", "style", "ariaHidden"),
    extend("svg", "xmlns", "width", "height", "viewBox", "preserveAspectRatio", "style"),
    ["path", ["d"]],
    ["line", ["x1", "y1", "x2", "y2", "strokeWidth"]],
    // Block structure the reader adds before sanitising.
    extend(
      "blockquote",
      ["className", "callout", ...CALLOUT_KINDS.map((kind) => `callout-${kind}`)],
      "dataLine",
      "dataCallout",
    ),
    extend("p", ["className", "callout-label"], "dataLine"),
    extend("div", ["className", "code-block", "mermaid"], "dataLine", "dataLang", "dataLines"),
    extend("pre", ["className", "shiki"], "dataLine"),
    extend("code", ["className", /^language-[\w+#.-]+$/, "math-inline", "math-display"]),
    extend("h1", "id", "dataLine"),
    extend("h2", "id", "dataLine", ["className", "sr-only"]),
    extend("h3", "id", "dataLine"),
    extend("h4", "id", "dataLine"),
    extend("h5", "id", "dataLine"),
    extend("h6", "id", "dataLine"),
    ...MATHML.map((tag) => [tag, MATHML_ATTRIBUTES] as [string, AttributeDefinition[]]),
  ]),
);

export const documentSchema: Schema = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames ?? []), "svg", "path", "line", ...MATHML],
  attributes,
  // `data:` stays out of `src`: local images become data: URLs only after sanitising, from
  // `document_asset` replies (render.ts).
  protocols: { ...defaultSchema.protocols, src: ["http", "https"] },
  // KaTeX's MathML annotation keeps its TeX source as text.
  strip: (defaultSchema.strip ?? []).filter((tag) => tag !== "annotation"),
};
