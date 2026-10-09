# Ultra 9000 icon sources

Built-in tools use their **real marks, unmodified**; Ultra 9000 designs only OMP's mark,
Shell, the generic agent, the provider fallback shapes, and its own app/tray icon. Every built-in can be replaced
by a user file: `[tools.<id>] icon = "icons/x.svg"` / `[providers.<id>] icon = …`, relative to `~/.config/ultra-9000/`.

## How real marks are used

- Originals live untouched in `brand-sources/` (see its README for sources and licences).
- Fitted copies (`tools/`, `providers/`, `providers/alt/`) wrap the original path data in one
  `<g transform="translate(x y) scale(s)">` — scale and translate only. No redrawing, cropping or path edits.
- Monochrome variants change fills only:
  - pi: yellow block → `currentColor` 100 %, pink → 80 %, blue → 55 % (lightness order of the original).
  - Neovim: both legs → `currentColor` at 60 %, diagonal → 100 %; gradients dropped, the original 13 % shadow kept.
  - Lobe Icons marks are single-colour already (`currentColor`); the spark's colour variant fills `#D97757`.
- Render each file at exactly its pixel box at an integer offset — never scale in CSS.

## Mapping

| Id / provider | Mark | Files | Notes |
| --- | --- | --- | --- |
| `omp` | Designed ⌥ (from OMP's banner motif) | `tools/omp-{12,14,18}.svg` | 2 px weight, 45° diagonal, whole-pixel bars |
| `claude-code` | Claude spark | `tools/claude-code-{12,14,18}[-color].svg` | fills the box; alt: `tools/alt/claude-code-mascot-*` |
| `codex` | Codex mark | `tools/codex-{12,14,18}.svg` | 10 / 12 / 16 px, 1 px in; alt (rejected): `tools/alt/codex-openai-knot-*` |
| `pi` | pi logo | `tools/pi-{12,14,18}[-color].svg` | 4 × 4 module grid on 3 / 3 / 4 px modules |
| `neovim` | Neovim mark | `tools/neovim-{12,14,18}[-color].svg` | fitted by height 12 / 14 / 16 |
| `shell`, `agent` | Designed | `tools/{shell,agent}-*.svg` | unchanged from round 1 |
| anthropic | Claude spark | `providers/anthropic-10.svg` | decided by the user (2026-10-04); not shown on Claude Code running a Claude model (same-maker rule). Rejected alt: “A\” in `providers/alt/anthropic-a-10.svg` |
| openai | OpenAI knot | `providers/openai-10.svg` | weakest at 10 px; readable as its ring |
| google | Gemini spark | `providers/google-10.svg` | |
| ollama, lmstudio, llamacpp | diamond | `providers/local-10.svg` | designed |
| anything else | ring | `providers/unknown-10.svg` | designed |

Mark boxes: 12 (20 px tile, padding 4; bare in 16 px), 14 (24 px tile, padding 5), 18 (30 px tile, padding 6).
Tile: `#2b2b2b`, 1 px inner `#3b3b3b`. Provider plate: 12 px, radius 3, row background, at (−5, −5); logo 10 px
mono `#b0bec5`; no plate on 20 px tiles. Real provider logos fail at 6 and 8 px (board 25).

Provider resolution (2026-10-04): the provider is the model's **maker**, not OMP's route prefix —
`openai-codex/gpt-6.1-sol` → openai; `amazon-bedrock/us.anthropic.claude-…` → anthropic;
`openrouter/moonshotai/kimi-k3` → moonshotai → ring unless `[providers.moonshotai] icon` is set. Local-runtime routes
(ollama, lmstudio, llama.cpp) keep the diamond.

Same-maker rule (user, 2026-10-04): hide the plate **and** the inline provider logo when the maker equals the tool's
own maker — `claude-code` + anthropic, `codex` + openai. The model name text stays. If Claude Code or Codex runs another
maker's model, the mark shows as usual. OMP, pi and custom tools always show it.

Colour (user, 2026-10-04): **every tile takes its tool colour by default** — sidebar, collapsed strip, context bar,
palette running and saved rows, detail pane, quit/confirm sheets, launch line, tool menu.
`icons.brand_color = "all" | "pickers" | "launch" | "off"`, default `all`. `pickers` keeps status lists mono
(`#e4ebee` marks) and colours only pick surfaces; `launch` colours only launch choices; `off` is mono.
Provider plates and inline provider logos stay mono everywhere; the same-maker rule applies (board 24, 25).

Tile (option D, user 2026-10-04 after real-desktop use): **one neutral tile for every tool** — `#2b2b2b` with a 1 px
`#3b3b3b` hairline, in every mode. Colour lives only in the mark; the earlier 14 % tile tint and 35 % hairline tint are
dropped.

| Tool | Asset (coloured surfaces) | Mark |
| --- | --- | --- |
| `omp` | `omp-*.svg`, CSS `color` | `#7fdbca` |
| `claude-code` | `claude-code-*-color.svg` | `#D97757` (art) |
| `codex` | `codex-*.svg` (mono by brand) | `#e4ebee` |
| `pi` | `pi-*-color.svg` | art |
| `neovim` | `neovim-*-color.svg` | art |
| `shell` | `shell-*.svg`, CSS `color` | `#c3e88d` |
| `agent` | `agent-*.svg`, CSS `color` | `#b0bec5` (or `[tools.x] accent`) |

Hover/selected change only the row background (`#272727` / `#323232`). The needs-you badge sits on a 2 px ring in the
row background, outside the mark, so it stays legible beside terracotta and mint marks (1×/4× proofs on board 24).
In sidebar rows the provider shows only as the tile's plate — no inline logo before the model name there (board 21).

## App (`app/`) and tray (`tray/`) — iteration 3

“U” followed by an **underscore cursor** on the U's baseline (iteration 2's tall block cursor read as “UI” and was
rejected). Drawn on a 16-unit grid: 8-wide U with 2-unit stems, 1-unit gap, 3 × 2 underscore. `ultra-9000-16.svg` is the
grid at 1 px/unit; `-32` is ×2; `-128` / `-256` / `-512` are ×8 / ×16 / ×32 — every edge on whole pixels. Tile radius
4 / 7 / 28 / 56 / 112, 1 px inner hairline; U `#eeffff`, cursor `#82aaff`. `ultra-9000-macos-1024.svg` places the same
grid on the macOS 1024/824 body.

Tray `tray-16`, `tray-20` (`currentColor` = mark colour): 3 px stems, underscore cursor; `-attention` adds the orange
dot top-right in the badge position (#ff9800; #e65100 on light bars) and changes nothing else. `macos-template-18.svg`
is the black template image (16 master, 1 px margin).

## Attribution (third-party notices)

- Icons from Lobe Icons, MIT License, © 2023 LobeHub (Claude spark, Anthropic, Codex, OpenAI, Gemini).
- Neovim logo by Jason Long, licensed under CC BY 3.0.
- pi logo from https://pi.dev — ask the pi project before public redistribution.
- All names and logos are trademarks of their owners, used only to identify the tool or provider being launched;
  no endorsement implied. Check each brand's guidelines before any public release.
