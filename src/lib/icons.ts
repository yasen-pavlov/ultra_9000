// Tool marks and provider logos (design boards 23–26, icons/README.md). Built-in marks are our own
// bundled SVGs, inlined so mono marks take currentColor; colour variants and user files are images.
import type { ToolInfo } from "./native";

/** Tile edge (16 = the bare mark, no tile) → mark box; app.css places it at padding 2 / 4 / 5 / 6. */
export type TileSize = 16 | 20 | 24 | 30;
export type MarkBox = 12 | 14 | 18;
export const MARK_BOX: Readonly<Record<TileSize, MarkBox>> = { 16: 12, 20: 12, 24: 14, 30: 18 };

const toolFiles = import.meta.glob<string>("./icons/tools/*.svg", { query: "?raw", import: "default", eager: true });
const providerFiles = import.meta.glob<string>("./icons/providers/*.svg", {
  query: "?raw",
  import: "default",
  eager: true,
});

// External ids (tool ids, provider prefixes, providerIcons keys) are only ever read through `own`:
// "constructor"/"toString" must not resolve to inherited object members.
function own<V>(record: Readonly<Record<string, V>>, key: string): V | undefined {
  return Object.hasOwn(record, key) ? record[key] : undefined;
}

function bundled(files: Record<string, string>, directory: string, name: string): string {
  const svg = own(files, `./icons/${directory}/${name}.svg`);
  if (svg === undefined) throw new Error(`missing bundled icon ${directory}/${name}.svg`);
  return svg;
}

/** Built-in tool id → mark file stem. */
const BUILTIN_MARK: Readonly<Record<string, string>> = {
  omp: "omp",
  claude: "claude-code",
  codex: "codex",
  pi: "pi",
  shell: "shell",
  neovim: "neovim",
};
/** Marks with a brand-colour artwork (used only where colour is allowed: launch choices and pickers). */
const COLOUR_MARKS: Readonly<Record<string, true>> = { "claude-code": true, neovim: true, pi: true };

export type ToolMark =
  /** Validated user file: never inlined; mono is a CSS mask of the same URL. */
  | { kind: "user"; url: string; tint: "mono" | "original"; fill: boolean }
  /** Bundled mono mark, inlined (currentColor); `generic` = the fallback agent mark, drawn neutral in pickers. */
  | { kind: "svg"; svg: string; generic?: true }
  /** Bundled colour artwork, shown as an image (keeps its own fills and gradient ids). */
  | { kind: "img"; url: string }
  /** Configured glyph letters in the UI font. */
  | { kind: "glyph"; text: string };

/** Resolution order: valid user file → built-in mark by tool id → configured glyph → generic agent mark. */
export function toolMark(toolId: string, tool: ToolInfo | undefined, box: MarkBox, colour: boolean): ToolMark {
  const icon = tool?.icon;
  if (icon) return { kind: "user", url: icon.url, tint: icon.tint, fill: icon.fill };
  const stem = own(BUILTIN_MARK, toolId);
  if (stem !== undefined) {
    if (colour && own(COLOUR_MARKS, stem) === true) {
      const artwork = bundled(toolFiles, "tools", `${stem}-${box}-color`);
      return { kind: "img", url: `data:image/svg+xml,${encodeURIComponent(artwork)}` };
    }
    return { kind: "svg", svg: bundled(toolFiles, "tools", `${stem}-${box}`) };
  }
  if (tool !== undefined && tool.glyph !== "") return { kind: "glyph", text: tool.glyph };
  return { kind: "svg", svg: bundled(toolFiles, "tools", `agent-${box}`), generic: true };
}

/** Model family by id ("claude-opus-5-5", Bedrock's "us.anthropic.claude-…", "gpt-6.1-sol", "gemini-…"). */
function familyOf(id: string): string {
  for (const part of id.toLowerCase().split(".")) {
    if (part.startsWith("claude")) return "anthropic";
    if (/^(gpt|o\d|codex|chatgpt)/.test(part)) return "openai";
    if (part.startsWith("gemini")) return "google";
  }
  return "";
}

/** Route prefixes that name a maker: "openai-codex" is OpenAI, "vertex" is Google. */
function makerOfRoute(route: string): string {
  const lower = route.toLowerCase();
  if (lower.startsWith("openai")) return "openai";
  if (lower.startsWith("anthropic")) return "anthropic";
  if (lower.startsWith("google") || lower.startsWith("gemini") || lower.startsWith("vertex")) return "google";
  return lower;
}

/**
 * The model's maker, which picks its provider logo. OMP records the route, not the maker
 * ("openai-codex/gpt-6.1-sol", "amazon-bedrock/us.anthropic.claude-…", "openrouter/moonshotai/kimi-k3"),
 * and Claude Code/Codex save bare ids. Order: a local runtime route (its models run locally whoever made
 * them: the diamond) → the model family read from the id → a route prefix that names a maker → the maker
 * segment of a three-part route ("moonshotai") → the route itself; "" when unknown.
 */
export function providerOf(model: string): string {
  const segments = model
    .trim()
    .split("/")
    .filter((segment) => segment !== "");
  const route = segments.length >= 2 ? makerOfRoute(segments[0]) : "";
  if (route === "ollama" || route === "lmstudio" || route === "llamacpp") return route;
  const family = familyOf(segments.at(-1) ?? "");
  if (family !== "" || segments.length < 2) return family;
  if (route === "openai" || route === "anthropic" || route === "google") return route;
  const maker = segments.length >= 3 ? segments.at(-2) : undefined;
  return maker?.toLowerCase() ?? route;
}

/** Tools whose maker is implied by the tool itself: their provider mark would only repeat the tool mark. */
const IMPLIED_PROVIDER: Readonly<Record<string, string>> = { claude: "anthropic", codex: "openai" };

/** Whether a provider mark adds information next to this tool (false for Claude Code on Anthropic, Codex on OpenAI). */
export function showsProvider(toolId: string, model: string): boolean {
  const implied = own(IMPLIED_PROVIDER, toolId);
  return implied === undefined || implied !== providerOf(model);
}

/** Built-in provider logos (board 25); local runtimes share the diamond, anything else is the ring. */
const BUILTIN_PROVIDER: Readonly<Record<string, string>> = {
  anthropic: "anthropic",
  openai: "openai",
  google: "google",
  ollama: "local",
  lmstudio: "local",
  llamacpp: "local",
};

export type ProviderMark = { kind: "user"; url: string } | { kind: "svg"; svg: string };

/** The 10 px provider logo for a model: a `[providers.<id>] icon` override, else the built-in map. */
export function providerMark(model: string, overrides: Readonly<Record<string, string>>): ProviderMark {
  const provider = providerOf(model);
  const url = provider === "" ? undefined : own(overrides, provider);
  if (url !== undefined) return { kind: "user", url };
  return {
    kind: "svg",
    svg: bundled(providerFiles, "providers", `${own(BUILTIN_PROVIDER, provider.toLowerCase()) ?? "unknown"}-10`),
  };
}
