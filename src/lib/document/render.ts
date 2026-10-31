// Markdown → sanitised HTML for the document surface (contract item 7). Native sends text, never
// HTML; everything here is pure apart from the injected asset loader. Order matters:
//   remark (GFM, math, frontmatter, callouts) → remark-rehype (raw HTML dropped) → KaTeX (only when
//   the text has math, lazily) → reader structure (slugs, data-line, code blocks, Shiki when loaded)
//   → rehype-sanitize (after every HTML-producing step) → images (https-only remote, local images as
//   data: URLs from `document_asset`) → HTML string.
// This module is itself loaded lazily by DocumentSurface, so none of it is in the initial bundle.
import type { HighlighterCore } from "@shikijs/core";
import GithubSlugger from "github-slugger";
import type { Element, ElementContent, Root as HastRoot } from "hast";
import { toHtml } from "hast-util-to-html";
import type { Blockquote, Paragraph, Root as MdastRoot, Text } from "mdast";
import rehypeSanitize from "rehype-sanitize";
import remarkFrontmatter from "remark-frontmatter";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified, type Pluggable } from "unified";
import { visit } from "unist-util-visit";
import { parseFrontmatter, type Frontmatter } from "./frontmatter";
import { CODE_THEME } from "./highlight";
import { documentSchema, type CalloutKind } from "./schema";

/** rehype-sanitize prefixes every `id` with this, so document ids never clobber the app's DOM. */
export const ID_PREFIX = "user-content-";

export interface DocumentHeading {
  level: number;
  text: string;
  /** GitHub-style slug, unique within the document; `path#slug` is the heading's link. */
  slug: string;
  /** DOM id of the rendered heading (`user-content-<slug>`). */
  id: string;
  /** 1-based source line. */
  line: number;
}

export interface RenderedDocument {
  html: string;
  /** Every heading in order (footnote labels excluded). */
  headings: DocumentHeading[];
  /** Text of the first H1, for the context bar and details pane. */
  h1: string | null;
  frontmatter: Frontmatter | null;
  /** Source line count, for “+N lines below”. */
  lineCount: number;
  /** Code blocks wait for Shiki: render again once `loadHighlighter()` resolves. */
  needsHighlighter: boolean;
}

export interface RenderOptions {
  /** `document_asset` as a data: URL; null when native refuses or the file is unreadable. */
  loadAsset: (relPath: string) => Promise<string | null>;
  /** Highlight code blocks with this; null renders plain code. */
  highlighter: HighlighterCore | null;
}

const CALLOUT_LABEL: Readonly<Record<CalloutKind, string>> = {
  note: "Note",
  tip: "Tip",
  warning: "Warning",
  important: "Important",
  caution: "Caution",
};
const CALLOUT_MARKER = /^\[!(note|tip|warning|important|caution)\][ \t]*(?:\r?\n)?/i;

/** Above this many characters of code, blocks stay plain so a huge log-style file renders quickly. */
const HIGHLIGHT_BUDGET = 400_000;
const NUMERIC_CELL = /^[-+−]?[\d.,\s]+%?$/;

function textOf(node: ElementContent | Element): string {
  if (node.type === "text") return node.value;
  if (node.type === "element") return node.children.map(textOf).join("");
  return "";
}

function classes(node: Element): string[] {
  const value = node.properties.className;
  return Array.isArray(value) ? value.map(String) : [];
}

/** `> [!NOTE]` … (board 31): a labelled blockquote; the marker line itself is removed. */
function remarkCallouts() {
  return (tree: MdastRoot) => {
    visit(tree, "blockquote", (quote: Blockquote) => {
      const first = quote.children[0];
      if (first?.type !== "paragraph") return;
      const lead = first.children[0];
      if (lead?.type !== "text") return;
      const match = CALLOUT_MARKER.exec(lead.value);
      if (match === null) return;
      const kind = match[1].toLowerCase() as CalloutKind;
      lead.value = lead.value.slice(match[0].length);
      if (lead.value === "") first.children.shift();
      const leadBreak = first.children[0];
      if (leadBreak?.type === "break") first.children.shift();
      if (first.children.length === 0) quote.children.shift();
      const label: Paragraph = {
        type: "paragraph",
        children: [{ type: "text", value: CALLOUT_LABEL[kind] } satisfies Text],
        data: { hProperties: { className: ["callout-label"] } },
      };
      quote.children.unshift(label);
      quote.data = { ...quote.data, hProperties: { className: ["callout", `callout-${kind}`], dataCallout: kind } };
    });
  };
}

function codeLanguage(code: Element): string | null {
  const language = classes(code).find((name) => name.startsWith("language-"));
  return language === undefined ? null : language.slice("language-".length);
}

/** Records a heading for the outline and gives it a slug id and its source line. */
function markHeading(node: Element, level: number, slugger: GithubSlugger, headings: DocumentHeading[]): void {
  const text = textOf(node).trim();
  const slug = slugger.slug(text);
  const line = node.position?.start.line ?? 0;
  node.properties.id = slug;
  node.properties.dataLine = line;
  headings.push({ level, text, slug, id: `${ID_PREFIX}${slug}`, line });
}

interface CodeState {
  highlighter: HighlighterCore | null;
  /** Characters of code still allowed through Shiki in this render. */
  budget: number;
  /** Set when a block waits for Shiki to load. */
  pending: { code: boolean };
}

/** A Shiki-highlighted copy of the block's <pre>, or the block itself (unknown language, over budget, no Shiki). */
function highlighted(pre: Element, language: string | null, source: string, state: CodeState): Element {
  if (language === null) return pre;
  if (state.highlighter === null) {
    state.pending.code = true;
    return pre;
  }
  if (!state.highlighter.getLoadedLanguages().includes(language) || source.length > state.budget) return pre;
  state.budget -= source.length;
  const root = state.highlighter.codeToHast(source.replace(/\n$/, ""), {
    lang: language,
    theme: CODE_THEME.name ?? "u9k",
  });
  const shiki = root.children.find((child): child is Element => child.type === "element");
  return shiki === undefined ? pre : { ...shiki, properties: { className: ["shiki"] } };
}

/** The wrapper a fenced block becomes: a Mermaid box, a code block, or null to leave it (math). */
function codeBlock(pre: Element, state: CodeState): Element | null {
  const code = pre.children.find((child): child is Element => child.type === "element" && child.tagName === "code");
  if (code === undefined) return null;
  const language = codeLanguage(code);
  if (language === "math") return null;
  const source = textOf(code);
  const line = pre.position?.start.line ?? 0;
  if (language === "mermaid") {
    const lines = source.replace(/\n$/, "").split("\n").length;
    return {
      type: "element",
      tagName: "div",
      properties: { className: ["mermaid"], dataLine: line, dataLines: lines },
      children: [pre],
    };
  }
  return {
    type: "element",
    tagName: "div",
    properties: { className: ["code-block"], dataLine: line, dataLang: language ?? "" },
    children: [highlighted(pre, language, source, state)],
  };
}

/** The reader's structure: heading slugs and lines, block lines, code and Mermaid wrappers, Shiki. */
function rehypeStructure(headings: DocumentHeading[], highlighter: HighlighterCore | null, pending: { code: boolean }) {
  return () => (tree: HastRoot) => {
    const slugger = new GithubSlugger();
    const state: CodeState = { highlighter, budget: HIGHLIGHT_BUDGET, pending };
    for (const child of tree.children) {
      if (child.type === "element" && child.position !== undefined)
        child.properties.dataLine = child.position.start.line;
    }
    visit(tree, "element", (node, index, parent) => {
      const level = /^h([1-6])$/.exec(node.tagName);
      // remark-rehype's own “Footnotes” label is not a document heading.
      if (level !== null) {
        if (!classes(node).includes("sr-only")) markHeading(node, Number(level[1]), slugger, headings);
        return;
      }
      if (node.tagName !== "pre" || parent === undefined || index === undefined) return;
      const block = codeBlock(node, state);
      if (block === null) return;
      parent.children[index] = block;
      return "skip";
    });
  };
}

function fallback(alt: string, url: string): Element {
  return {
    type: "element",
    tagName: "span",
    properties: { className: ["img-fallback"] },
    children: [{ type: "text", value: [alt, url].filter((part) => part !== "").join(" · ") }],
  };
}

function isRelative(src: string): boolean {
  return !/^[a-z][a-z0-9+.-]*:/i.test(src) && !src.startsWith("//") && !src.startsWith("#");
}

/**
 * After sanitising: remote images load only over https and without a referrer; anything else with
 * a scheme (http:, …) is replaced by its alt text and URL; relative paths become data: URLs from
 * the asset loader (or the same fallback when native refuses them).
 */
async function resolveImages(tree: HastRoot, loadAsset: RenderOptions["loadAsset"]): Promise<void> {
  const images: { node: Element; parent: Element | HastRoot; index: number }[] = [];
  visit(tree, "element", (node, index, parent) => {
    if (node.tagName === "img" && parent !== undefined && index !== undefined) images.push({ node, parent, index });
  });
  const relative = new Map<string, Promise<string | null>>();
  await Promise.all(
    images.map(async ({ node, parent, index }) => {
      const src = typeof node.properties.src === "string" ? node.properties.src : "";
      const alt = typeof node.properties.alt === "string" ? node.properties.alt : "";
      if (/^https:/i.test(src)) {
        node.properties.referrerPolicy = "no-referrer";
        node.properties.loading = "lazy";
        node.properties.decoding = "async";
        return;
      }
      if (src === "" || !isRelative(src)) {
        parent.children[index] = fallback(alt, src);
        return;
      }
      let path = src.split(/[?#]/)[0];
      try {
        path = decodeURIComponent(path);
      } catch {
        // Keep the literal path; native resolves or refuses it.
      }
      let pending = relative.get(path);
      if (pending === undefined) {
        pending = loadAsset(path).catch(() => null);
        relative.set(path, pending);
      }
      const url = await pending;
      if (url?.startsWith("data:") === true) node.properties.src = url;
      else parent.children[index] = fallback(alt, src);
    }),
  );
}

/** Numbers in tables are mono and right-aligned (board 31). */
function markNumericCells(tree: HastRoot): void {
  visit(tree, "element", (node) => {
    if (node.tagName !== "td") return;
    const text = textOf(node).trim();
    if (text !== "" && NUMERIC_CELL.test(text)) node.properties.className = [...classes(node), "num"];
  });
}

function hasMath(tree: MdastRoot): boolean {
  let found = false;
  visit(tree, (node) => {
    if (node.type === "math" || node.type === "inlineMath") {
      found = true;
      return false;
    }
  });
  return found;
}

let katexPlugin: Promise<Pluggable> | null = null;

/** rehype-katex with its stylesheet and fonts, fetched the first time a document has math. */
function loadKatex(): Promise<Pluggable> {
  katexPlugin ??= Promise.all([import("rehype-katex"), import("katex/dist/katex.min.css")]).then(([module]) => [
    module.default,
    { output: "htmlAndMathml", strict: "ignore" },
  ]);
  return katexPlugin;
}

function takeFrontmatter(tree: MdastRoot): Frontmatter | null {
  const node = tree.children[0];
  if (node === undefined || !("value" in node)) return null;
  // remark-frontmatter emits `yaml` and (untyped in mdast) `toml` nodes.
  const type: string = node.type;
  if (type !== "yaml" && type !== "toml") return null;
  tree.children.shift();
  return parseFrontmatter(type, node.value);
}

const parser = unified().use(remarkParse).use(remarkFrontmatter, ["yaml", "toml"]).use(remarkGfm).use(remarkMath);

export async function renderMarkdown(text: string, options: RenderOptions): Promise<RenderedDocument> {
  const mdast = parser.parse(text);
  const frontmatter = takeFrontmatter(mdast);
  const headings: DocumentHeading[] = [];
  const pending = { code: false };
  const plugins: Pluggable[] = [remarkCallouts, remarkRehype];
  if (hasMath(mdast)) plugins.push(await loadKatex());
  plugins.push(rehypeStructure(headings, options.highlighter, pending), [rehypeSanitize, documentSchema]);
  const hast = (await unified().use(plugins).run(mdast)) as HastRoot;
  await resolveImages(hast, options.loadAsset);
  markNumericCells(hast);
  return {
    html: toHtml(hast),
    headings,
    h1: headings.find((heading) => heading.level === 1)?.text ?? null,
    frontmatter,
    lineCount: text.split("\n").length,
    needsHighlighter: pending.code,
  };
}
