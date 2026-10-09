import { describe, expect, it } from "vitest";
import agent14 from "./icons/tools/agent-14.svg?raw";
import claudeCode18Color from "./icons/tools/claude-code-18-color.svg?raw";
import claudeCode18 from "./icons/tools/claude-code-18.svg?raw";
import codex12Svg from "./icons/tools/codex-12.svg?raw";
import omp12 from "./icons/tools/omp-12.svg?raw";
import anthropic10 from "./icons/providers/anthropic-10.svg?raw";
import google10 from "./icons/providers/google-10.svg?raw";
import local10 from "./icons/providers/local-10.svg?raw";
import openai10 from "./icons/providers/openai-10.svg?raw";
import unknown10 from "./icons/providers/unknown-10.svg?raw";
import { MARK_BOX, providerMark, providerOf, showsProvider, toolMark } from "./icons";
import { toolInfo as tool } from "../test/fixtures";

describe("MARK_BOX", () => {
  it("only maps tiles to boxes every built-in mark ships artwork for, mono and colour", () => {
    for (const box of Object.values(MARK_BOX)) {
      for (const id of ["omp", "claude", "codex", "pi", "shell", "neovim"]) {
        expect(toolMark(id, undefined, box, false).kind).toBe("svg");
        expect(["svg", "img"]).toContain(toolMark(id, undefined, box, true).kind);
      }
    }
  });
});

describe("toolMark", () => {
  it("prefers a validated user icon over everything else, keeping its tint and fill", () => {
    const icon = { url: "data:image/png;base64,AAAA", tint: "original" as const, fill: true };
    expect(toolMark("omp", tool({ icon, glyph: "Om" }), 12, true)).toEqual({
      kind: "user",
      url: icon.url,
      tint: "original",
      fill: true,
    });
  });

  it("inlines the built-in mono mark of the requested box for built-in tools", () => {
    expect(toolMark("omp", tool(), 12, false)).toEqual({ kind: "svg", svg: omp12 });
    expect(toolMark("claude", undefined, 18, false)).toEqual({ kind: "svg", svg: claudeCode18 });
  });

  it("uses colour artwork as an image only where colour is allowed and the mark has one", () => {
    const mark = toolMark("claude", undefined, 18, true);
    expect(mark).toEqual({
      kind: "img",
      url: `data:image/svg+xml,${encodeURIComponent(claudeCode18Color)}`,
    });
    // Codex has no colour variant: stays an inlined mono mark even when colour is allowed.
    expect(toolMark("codex", undefined, 12, true)).toEqual({ kind: "svg", svg: codex12Svg });
  });

  it("draws a configured glyph for custom tools without a mark", () => {
    expect(toolMark("aider", tool({ id: "aider", glyph: "Ai" }), 14, true)).toEqual({ kind: "glyph", text: "Ai" });
  });

  it("falls back to the generic agent mark, flagged generic, for unknown tools without a glyph", () => {
    const expected = { kind: "svg", svg: agent14, generic: true };
    expect(toolMark("aider", tool({ id: "aider" }), 14, false)).toEqual(expected);
    expect(toolMark("aider", undefined, 14, false)).toEqual(expected);
  });

  it("does not resolve inherited object members as built-in tool ids", () => {
    expect(toolMark("constructor", undefined, 14, true)).toEqual({ kind: "svg", svg: agent14, generic: true });
    expect(toolMark("toString", tool({ glyph: "T" }), 14, true)).toEqual({ kind: "glyph", text: "T" });
  });
});

describe("providerOf", () => {
  it.each([
    // Bare ids saved by Claude Code / Codex: the model family.
    ["claude-opus-5-5", "anthropic"],
    ["gpt-6.1-sol", "openai"],
    ["o4-mini", "openai"],
    ["codex-mini", "openai"],
    ["gemini-3-pro", "google"],
    ["kimi-k3", ""],
    ["", ""],
    // OMP routes: the family wins over the route.
    ["openai-codex/gpt-6.1-sol", "openai"],
    ["amazon-bedrock/us.anthropic.claude-sonnet-4-6", "anthropic"],
    ["openrouter/anthropic/claude-opus-5-5", "anthropic"],
    // Local runtimes keep their route whoever made the model.
    ["ollama/llama-4", "ollama"],
    ["lmstudio/claude-distill", "lmstudio"],
    ["llamacpp/qwen3", "llamacpp"],
    // Unknown family: a route that names a maker, else the maker segment, else the route.
    ["anthropic-vertex/some-model", "anthropic"],
    ["vertex/some-model", "google"],
    ["gemini-api/some-model", "google"],
    ["openrouter/moonshotai/kimi-k3", "moonshotai"],
    ["openrouter/MoonshotAI/kimi-k3", "moonshotai"],
    ["Groq/llama-4", "groq"],
    ["  /mistral/  ", ""],
  ])("%s → %s", (model, expected) => {
    expect(providerOf(model)).toBe(expected);
  });
});

describe("showsProvider", () => {
  it("hides the provider mark only where the tool already implies that maker", () => {
    expect(showsProvider("claude", "claude-opus-5-5")).toBe(false);
    expect(showsProvider("codex", "gpt-6.1-sol")).toBe(false);
    expect(showsProvider("claude", "openrouter/moonshotai/kimi-k3")).toBe(true);
    expect(showsProvider("codex", "claude-opus-5-5")).toBe(true);
    expect(showsProvider("omp", "anthropic/claude-opus-5-5")).toBe(true);
  });

  it("does not treat inherited object members as tools with an implied maker", () => {
    expect(showsProvider("constructor", "")).toBe(true);
  });
});

describe("providerMark", () => {
  it.each([
    ["anthropic/claude-opus-5-5", anthropic10],
    ["gpt-6.1", openai10],
    ["gemini-3-pro", google10],
    ["ollama/llama-4", local10],
    ["lmstudio/qwen3", local10],
    ["llamacpp/qwen3", local10],
    ["openrouter/moonshotai/kimi-k3", unknown10],
    ["kimi-k3", unknown10],
  ])("%s uses the matching built-in logo", (model, svg) => {
    expect(providerMark(model, {})).toEqual({ kind: "svg", svg });
  });

  it("uses a configured provider icon override for that provider only", () => {
    const overrides = { moonshotai: "data:image/png;base64,MOON" };
    expect(providerMark("openrouter/moonshotai/kimi-k3", overrides)).toEqual({
      kind: "user",
      url: overrides.moonshotai,
    });
    expect(providerMark("gpt-6.1", overrides)).toEqual({ kind: "svg", svg: openai10 });
  });

  it("never applies an override to a model with no known provider", () => {
    expect(providerMark("kimi-k3", { "": "data:image/png;base64,X" })).toEqual({ kind: "svg", svg: unknown10 });
  });

  it("ignores inherited object members as provider ids", () => {
    expect(providerMark("router/constructor/model", {})).toEqual({ kind: "svg", svg: unknown10 });
  });
});
