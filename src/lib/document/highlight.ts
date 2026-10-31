// Code highlighting for documents: Shiki core with a fine-grained grammar set and the JavaScript
// regex engine (no WASM), loaded on first use so it never weighs on the initial bundle. Colours are
// the terminal's (src-tauri/src/alacritty.toml) on the app's well (app.css), so a code block looks
// like the terminal next door (board 31). Orange (--accent) never appears.
import type { HighlighterCore, ThemeRegistrationRaw } from "@shikijs/core";

/** Terminal palette (alacritty.toml) and app tokens (app.css) the theme is built from. */
export const CODE_PALETTE = {
  well: "#1c1c1c", // --well
  fg: "#eeffff", // --fg, terminal foreground
  dim: "#8c8b8b", // --dim
  text: "#b0bec5", // --text
  blue: "#82aaff",
  cyan: "#89ddff",
  green: "#c3e88d",
  magenta: "#c792ea",
  red: "#f07178",
  yellow: "#ffcb6b",
  brightBlue: "#b2ccd6",
} as const;

const p = CODE_PALETTE;

export const CODE_THEME: ThemeRegistrationRaw = {
  name: "u9k",
  type: "dark",
  colors: { "editor.background": p.well, "editor.foreground": p.fg },
  settings: [
    { settings: { foreground: p.fg, background: p.well } },
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: p.dim, fontStyle: "italic" } },
    {
      scope: ["keyword", "storage", "storage.type", "keyword.control", "keyword.operator.new"],
      settings: { foreground: p.magenta },
    },
    {
      scope: ["keyword.operator", "punctuation.separator", "punctuation.accessor", "meta.brace"],
      settings: { foreground: p.cyan },
    },
    { scope: ["string", "string.quoted", "markup.inline.raw", "string.template"], settings: { foreground: p.green } },
    { scope: ["constant.character.escape", "string.regexp"], settings: { foreground: p.cyan } },
    {
      scope: ["constant.numeric", "constant.language", "constant.other", "support.constant"],
      settings: { foreground: p.red },
    },
    {
      scope: ["entity.name.function", "support.function", "meta.function-call", "variable.function"],
      settings: { foreground: p.blue },
    },
    {
      scope: ["entity.name.type", "entity.name.class", "support.type", "support.class", "entity.other.inherited-class"],
      settings: { foreground: p.yellow },
    },
    { scope: ["entity.name.tag", "meta.tag"], settings: { foreground: p.red } },
    { scope: ["entity.other.attribute-name"], settings: { foreground: p.magenta } },
    {
      scope: ["variable.parameter", "variable.other.property", "support.variable.property"],
      settings: { foreground: p.text },
    },
    { scope: ["variable.language", "variable.other.constant"], settings: { foreground: p.red } },
    { scope: ["markup.heading", "entity.name.section"], settings: { foreground: p.blue, fontStyle: "bold" } },
    { scope: ["markup.bold"], settings: { fontStyle: "bold" } },
    { scope: ["markup.italic"], settings: { fontStyle: "italic" } },
    { scope: ["markup.underline.link", "string.other.link"], settings: { foreground: p.cyan } },
    {
      scope: ["markup.inserted", "meta.diff.header.to-file", "punctuation.definition.inserted"],
      settings: { foreground: p.green },
    },
    {
      scope: ["markup.deleted", "meta.diff.header.from-file", "punctuation.definition.deleted"],
      settings: { foreground: p.red },
    },
    { scope: ["meta.diff.range", "meta.diff.header"], settings: { foreground: p.brightBlue } },
  ],
};

/** Grammars bundled for documents (contract item 7); anything else renders as plain mono text. */
const GRAMMARS = [
  () => import("@shikijs/langs/typescript"),
  () => import("@shikijs/langs/javascript"),
  () => import("@shikijs/langs/rust"),
  () => import("@shikijs/langs/python"),
  () => import("@shikijs/langs/shellscript"),
  () => import("@shikijs/langs/json"),
  () => import("@shikijs/langs/yaml"),
  () => import("@shikijs/langs/toml"),
  () => import("@shikijs/langs/markdown"),
  () => import("@shikijs/langs/html"),
  () => import("@shikijs/langs/css"),
  () => import("@shikijs/langs/svelte"),
  () => import("@shikijs/langs/lua"),
  () => import("@shikijs/langs/sql"),
  () => import("@shikijs/langs/diff"),
  () => import("@shikijs/langs/go"),
  () => import("@shikijs/langs/c"),
  () => import("@shikijs/langs/cpp"),
];

let loading: Promise<HighlighterCore> | null = null;
let ready: HighlighterCore | null = null;

/** The highlighter once loaded, else null (the caller renders plain code and asks for a reload). */
export function loadedHighlighter(): HighlighterCore | null {
  return ready;
}

/** Loads Shiki once per app run; later calls share the same promise. */
export function loadHighlighter(): Promise<HighlighterCore> {
  loading ??= (async () => {
    const [{ createHighlighterCore }, { createJavaScriptRegexEngine }] = await Promise.all([
      import("@shikijs/core"),
      import("@shikijs/engine-javascript"),
    ]);
    const highlighter = await createHighlighterCore({
      themes: [CODE_THEME],
      langs: GRAMMARS.map((grammar) => grammar()),
      engine: createJavaScriptRegexEngine(),
    });
    ready = highlighter;
    return highlighter;
  })();
  loading.catch(() => {
    loading = null;
  });
  return loading;
}
