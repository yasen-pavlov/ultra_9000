# Configuration

Settings are files you can keep in git, at `~/.config/ultra-9000/` on Linux and macOS (or `$XDG_CONFIG_HOME/ultra-9000`):

- `ultra.toml`: launch defaults, sidebar, notifications, terminals, `[projects]` (pinned, roots, per-project `default_tool`/`name`) and `[tools.<id>]` agent definitions.
- `terminal.toml`: the embedded terminal's font and colors, in Alacritty config syntax. It is merged over the bundled defaults; your global Alacritty config is never read.
- `keys.toml`: `[bindings]` shortcut overrides (`action = "Ctrl+Shift+X"`); see [Keyboard shortcuts](#keyboard-shortcuts).
- `machines/<hostname>.toml`: per-machine overrides. Set `ULTRA_9000_MACHINE` to choose a different file.

Changes apply live where possible. An invalid key keeps its last good value and shows a banner in Settings (Ctrl+Shift+, / Cmd+,). The settings sheet edits a small subset in place and preserves comments. UI memory (sidebar state, folded projects, the last quit's agent set) lives in `~/.local/state/ultra-9000/state.json` (`$XDG_STATE_HOME/ultra-9000/state.json`) and is not a setting.

## Single-instance launch

A second launch shows, unminimizes, raises and focuses the running window instead of starting another app or resuming its sessions again. On Wayland, the launcher forwards `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID` when available; activation is best effort and still depends on compositor focus policy.

Before GUI initialization, the app takes a non-blocking exclusive `flock` on `instance.lock` in the control link's private runtime directory (Linux: `$XDG_RUNTIME_DIR/ultra-9000`, or a per-user directory in the temp directory; macOS: `<per-user temp>/ultra-9000`). The owner writes a complete `<pid> <lock filename>` record and holds the fd until exit; the kernel releases the lock after a crash, so an existing lock file is not a stale lock and must not be deleted.

If the owner is still starting or quitting, the launcher retries for up to 5 s. Each attempt first tries to acquire the same lock; if the previous process has exited, the launcher becomes the new owner and starts normally. Otherwise it reads a complete record for the matching key and sends a show command to `control-<pid>.sock`. Disconnects, unavailable sockets and rejected handovers (including `quitting`) are retried within that deadline. A successful handover exits 0 and prints `Ultra 9000 is already running (pid N); showing its window.` to stderr; exhausting the deadline exits 1 with an explanation, never starting without the lock. An ordinary release launch also hands over to a live pre-lock v0.3.0 control socket, excluding every pid that owns a held `instance*.lock` (so newer debug/isolated instances are not legacy owners). v0.2.0 has no control socket and cannot be detected.

Debug builds use `instance-debug.lock` and do not hand over to the installed release app. For an isolated run of either build profile, set `ULTRA9000_INSTANCE=<name>`: 1–32 characters drawn from lowercase ASCII letters, digits, `_` and `-`; invalid names fail before GUI startup. This selects `instance-<name>.lock` and disables the pre-lock fallback. Use a unique name for native test harnesses, alongside isolated config/state/data directories (the name isolates the instance lock, not settings or session stores). `ultra9000 ctl` never takes an instance lock.

Startup fails rather than starting an unprotected instance if the runtime directory is not private. The error names its path, owner uid and mode: it must be a non-symlink directory owned by your uid, with no group/other access (normally mode 0700). Set `XDG_RUNTIME_DIR` on Linux, or `TMPDIR` on macOS, to a private directory owned by you. If the app's runtime directory is abandoned or squatted, remove it only after ensuring no Ultra 9000 instance is using it; never delete a live lock file.

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

## Tray icon and shell plugins

```toml
[tray]
show = "auto" # auto (default) | always | never
```

`never` removes the tray icon (Linux StatusNotifierItem, macOS menu-bar item) and `always` keeps it. `auto` shows it except while a desktop-shell plugin watches the app over the control link below: the icon is withdrawn when the first watcher connects and returns 5 s after the last one disconnects, so a crashed or removed plugin never leaves a hidden window without a way back. Changes apply live.

### Control link

The running app listens on a private local socket (`control-<pid>.sock` next to the OMP status link: `$XDG_RUNTIME_DIR/ultra-9000/` on Linux, the per-user temp directory on macOS) that a shell plugin such as Noctalia's `muty/ultra9000` uses through the app's own command-line client, which never starts the app:

```sh
ultra9000 ctl watch [--client NAME]       # prints the app's lines (newline-delimited JSON) until stdout closes
ultra9000 ctl select_item '{"itemId":"item-3"}'
ultra9000 ctl jump_to_waiting
ultra9000 ctl set_window_visible '{"visible":"toggle"}'
ultra9000 ctl set_window_visible '{"visible":true,"activationToken":"<XDG_ACTIVATION_TOKEN>","desktopStartupId":"<DESKTOP_STARTUP_ID>"}' # activation fields optional
ultra9000 ctl new_item '{"projectId":"/abs/project","kind":"shell"}'   # kind: agent | shell | neovim, optional tool
ultra9000 ctl open_palette '{"projectId":"/abs/project"}'
ultra9000 ctl close_item '{"itemId":"item-3","confirmed":false}'
ultra9000 ctl request_quit
```

`watch` prints a `snapshot` of the sidebar (projects and items in sidebar order with their state, no icons) at once and after every change, a `ping` every 5 s, and `{"type":"disconnected"}` once whenever no app is running or the app goes away; it reconnects every 1 s, backing off to 10 s. A one-shot command prints its `reply` and exits 0 if it succeeded, 1 if not, or 2 when no app is running (printing `{"type":"disconnected"}`) or the arguments are not a JSON object. `close_item` follows the app's close rules but never opens the app's confirmation; it answers `"result":"needs-confirmation"` with a `reason` instead, and `"confirmed":true` closes. The newest running instance is used; `ULTRA9000_CONTROL_SOCKET=<path>` picks a socket explicitly. Inside the macOS bundle the binary is `Ultra 9000.app/Contents/MacOS/ultra9000`.

`set_window_visible` accepts optional string fields `activationToken` and `desktopStartupId`, each at most 4096 bytes and without NUL (invalid values return `bad_request`). When showing the window it also unminimizes, raises and requests focus, even if already visible; Linux passes the activation token (preferred), or the startup id, to GTK 3 before presenting the window. Missing activation fields are valid; fields have no effect when hiding and are ignored on macOS. Once quit begins, the command returns `quitting`; if saving fails and quitting is cancelled it becomes available again. The single-instance launcher uses this existing command with `visible:true` and a non-watching hello, so handover does not suppress the tray; older v0.3.0 owners ignore the added fields.

The single-instance launcher drops invalid activation environment values instead of forwarding them: showing the window still succeeds without activation. Direct control commands with invalid activation fields continue to return `bad_request`.

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
