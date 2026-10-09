# Configuration

Settings are files you can keep in git, at `~/.config/ultra-9000/` on Linux and macOS (or `$XDG_CONFIG_HOME/ultra-9000`):

- `ultra.toml`: launch defaults, sidebar, notifications, terminals, `[projects]` (pinned, roots, per-project `default_tool`/`name`) and `[tools.<id>]` agent definitions.
- `terminal.toml`: the embedded terminal's font and colors, in Alacritty config syntax. It is merged over the bundled defaults; your global Alacritty config is never read.
- `keys.toml`: `[bindings]` shortcut overrides (`action = "Ctrl+Shift+X"`); see [Keyboard shortcuts](#keyboard-shortcuts).
- `machines/<hostname>.toml`: per-machine overrides. Set `ULTRA_9000_MACHINE` to choose a different file.

Changes apply live where possible. An invalid key keeps its last good value and shows a banner in Settings (Ctrl+Shift+, / Cmd+,). The settings sheet edits a small subset in place and preserves comments. UI memory (sidebar state, folded projects, the last quit's agent set) lives in `~/.local/state/ultra-9000/state.json` (`$XDG_STATE_HOME/ultra-9000/state.json`) and is not a setting.

## Keyboard shortcuts

App shortcuts work while the terminal or the web view has focus. They match the **physical key**, so they work under any keyboard layout. Override them in `keys.toml` with the action names below, for example:

```toml
[bindings]
palette = "Ctrl+Shift+P"
new-neovim = "" # unbind
```

| Action (`keys.toml` name) | Linux | macOS |
| --- | --- | --- |
| Palette: switch, resume, start (`palette`) | Ctrl+Shift+Space | Cmd+O |
| Palette scoped to the current project (`palette-project`) | Ctrl+Shift+R | Cmd+R |
| New agent here, project default tool (`new-agent`) | Ctrl+Shift+N | Cmd+N |
| New terminal below the active item (`new-terminal`) | Ctrl+Shift+T | Cmd+T |
| New Neovim here (`new-neovim`) | Ctrl+Shift+E | Cmd+E |
| Close item, again to confirm (`close`) | Ctrl+Shift+W | Cmd+W |
| Jump to what's waiting: needs-you first, then unread your-turn (`jump-waiting`) | Ctrl+Shift+J | Cmd+J |
| Item 1…9 in visible order (`item-1` … `item-9`) | Ctrl+Shift+1…9 | Cmd+1…9 |
| Previous / next item (`item-previous`, `item-next`) | Ctrl+Shift+Up / Down | Cmd+Shift+[ / ] |
| Previous / next project (`project-previous`, `project-next`) | Ctrl+Shift+PageUp / PageDown | Cmd+Option+[ / ] |
| Focus sidebar (`focus-sidebar`) | Ctrl+Shift+L | Cmd+L |
| Expand / collapse sidebar (`toggle-sidebar`) | Ctrl+Shift+S | Ctrl+Cmd+S |
| Settings (`settings`) | Ctrl+Shift+, | Cmd+, |
| All shortcuts (`shortcuts`) | Ctrl+Shift+/ | Cmd+/ |
| Hide window (`hide`) | Ctrl+Shift+H | Cmd+H |
| Quit (`quit`) | Ctrl+Shift+Q | Cmd+Q |

On Linux, Ctrl+PageUp / PageDown also select the previous / next item.

**Never captured:** these keys always reach the terminal (or the compositor). An override that uses one is ignored and reported as a settings error.

- Linux: every chord without Ctrl+Shift (unshifted Ctrl/Alt chords, Shift+PageUp/PageDown, plain keys), except the Ctrl+PageUp/PageDown aliases above; every Super chord; Ctrl+Shift+C/V/F/B/[ (Alacritty), Ctrl+Shift+O and P (OMP), Ctrl+Shift+I, K and Print (compositor), Ctrl+Shift+= and - (font size) and Ctrl+Shift+Escape.
- macOS: every chord without Cmd; Cmd+C/V/F/B/K, Cmd+Escape and Cmd+Print. Left Option stays Alt.

The bundled terminal config unbinds Alacritty's Ctrl+Shift+Space vi-mode default; vi mode stays on Ctrl+Shift+[.

## Tool, provider and tray icons

```toml
[sidebar]
provider_mark = "corner" # corner | off; default corner, no plate on compact rows
[icons]
brand_color = "all" # all (default) | pickers | launch | off: where tool marks show colour
[tray]
icon = "light" # light | dark; light marks for dark bars (default)

[tools.claude]
icon = "icons/claude.svg"
icon_tint = "mono" # mono (default) | original
icon_fill = false # fit mark box (default); true fills the tile
accent = "#7fdbca" # colour of single-colour marks; built-ins: omp #7fdbca, shell #c3e88d

[tools.custom]
command = "custom"
# glyph = "ag" # optional UI-font fallback; omit to use the generic agent mark

[providers.anthropic]
icon = "icons/provider.png" # overrides the built-in provider mark, always mono
```

Icon files resolve relative to the config directory. SVG/PNG content is validated regardless of extension, at most 256 KB; absolute paths, `..`, non-files and symlinks outside the config directory are rejected. Missing/invalid icons fall back without blocking launch and show a per-file settings error; editing or adding an icon hot-reloads it. Resolution is valid user file → built-in tool mark → configured glyph → generic agent mark. Tool identity is not replaced by the model: the old icon-mode key is removed, with no compatibility alias.

## Provider resolution

The provider mark shows the model's maker, not the route it runs through. Resolution order:

1. A local runtime route stays that runtime: `ollama/gpt-oss` gets the diamond.
2. The model family read from the id: `claude*` (including Bedrock's `us.anthropic.claude-…`) is anthropic, `gpt*`/`o<digit>*`/`codex*`/`chatgpt*` is openai, and `gemini*` is google.
3. A route prefix that names a maker: `openai-codex/…` is openai, `vertex/…` is google.
4. The middle segment of a three-part route: `openrouter/moonshotai/kimi-k3` is `moonshotai`.
5. The prefix itself.

`[providers.<id>] icon` keys use the resolved id. The built-in marks: anthropic uses the Claude spark, openai the OpenAI mark, google Gemini, ollama/lmstudio/llamacpp a diamond, and anything else a ring. Provider logos are 10 px mono. Corner plates appear only on comfortable and sheet tiles, and inline marks appear beside model names in the palette and context bar. Neither is shown when the tool already implies the maker (Claude Code on Anthropic, Codex on OpenAI); the model name stays.

## Regenerating bundled icons

Regenerate the app/hicolor/ICNS and tray PNG families with `python3 scripts/icons.py` (requires Python 3, `rsvg-convert` and ImageMagick `magick`). The SVG masters are in `assets/icons/` (see its README for how the tool and provider marks are fitted); generated assets are committed. Third-party marks and their licenses are listed in [third-party notices](../THIRD_PARTY_NOTICES.md).
