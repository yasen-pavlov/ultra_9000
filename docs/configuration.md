# Configuration

Settings are files you can keep in git, at `~/.config/ultra-9000/` on Linux and macOS (or `$XDG_CONFIG_HOME/ultra-9000`):

- `ultra.toml`: launch defaults, sidebar, notifications, power, terminals, `[projects]` (pinned, roots, per-project `default_tool`/`name`) and `[tools.<id>]` agent definitions.
- `terminal.toml`: the embedded terminal's font and colors, in Alacritty config syntax. It is merged over the bundled defaults; your global Alacritty config is never read.
- `keys.toml`: `[bindings]` shortcut overrides (`action = "Ctrl+Shift+X"`); see [Keyboard shortcuts](#keyboard-shortcuts).
- `machines/<hostname>.toml`: per-machine overrides. Set `ULTRA_9000_MACHINE` to choose a different file.

Changes apply live where possible. An invalid key keeps its last good value and shows a banner in Settings (Ctrl+Shift+, / Cmd+,). The settings sheet edits a small subset in place and preserves comments. UI memory (sidebar state, folded projects, the last quit's agent set) lives in `~/.local/state/ultra-9000/state.json` (`$XDG_STATE_HOME/ultra-9000/state.json`) and is not a setting.

## Notifications

```toml
[notify]
needs_you = true
turn_finished = false
```

Notifications are sent as **Ultra 9000**, with its app identity, when an agent needs you or finishes a turn while its item is not presented in the focused main window. Clicking a notification shows the window and selects that item; if the item has closed, it just shows the window. A newer notification for an item replaces its older one. On Wayland, focus activation uses the notification server's token when available and remains subject to compositor policy.

macOS asks for notification permission once; change the choice in System Settings → Notifications → Ultra 9000. Notifications need a signed `.app` bundle: release builds are; a development run sends none, and a locally built bundle needs at least an ad-hoc signature (`codesign --force --deep -s - "Ultra 9000.app"`).

## Keep awake

```toml
[power]
keep_awake = "off" # off (default) | working | running
```

This live setting is also available in the settings sheet:

- `off`: do not inhibit idle sleep.
- `working`: inhibit while any OMP-monitored agent reports **working** or **on job**. Needs-you and your-turn do not count; exited and failed items never count. Activity is never inferred from output or CPU.
- `running`: inhibit while any item is starting or running, including idle agents, shells and Neovim.

On **macOS**, Ultra 9000 holds an IOKit `PreventUserIdleSystemSleep` assertion. The display may still sleep, and closing the laptop lid still puts it to sleep. Activity Monitor's **Preventing Sleep** column shows the assertion.

On **Linux**, Ultra 9000 holds a logind `idle` inhibitor labelled **Ultra 9000 keep awake**, honoured by idle daemons such as hypridle. **This also stops idle screen locking while held**; it does not prohibit manual locking or explicit suspend. Inspect it with `systemd-inhibit --list`. The macOS assertion uses the same neutral label, so switching enabled modes never leaves stale reason text in OS tools.

The Linux inhibitor descriptors are marked close-on-exec as soon as they are received by the inhibitor worker, so launched shells and tools do not inherit an established hold. A small cross-thread window remains between D-Bus receiving a descriptor and the worker marking it during acquisition.

Hiding the app does not release an inhibitor while the selected condition still holds. It is released when the condition stops, the setting becomes `off`, or the app exits. Switching enabled modes retains an existing hold. Failed acquisitions retry on later workspace changes after a 30-second backoff, logging only the first consecutive failure; the app keeps running. Turning the setting off and back on permits an immediate new attempt.

## Single-instance launch

A second launch shows, unminimizes, raises and focuses the running window instead of starting another app or resuming its sessions again. On Wayland, the launcher forwards `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID` when available; activation is best effort and still depends on compositor focus policy.

Before GUI initialization, the app takes a non-blocking exclusive `flock` on `instance.lock` in the control link's private runtime directory (Linux: `$XDG_RUNTIME_DIR/ultra-9000`, or a per-user directory in the temp directory; macOS: `<per-user temp>/ultra-9000`). The owner writes a complete `<pid> <lock filename>` record and holds the fd until exit; the kernel releases the lock after a crash, so an existing lock file is not a stale lock and must not be deleted.

If the owner is still starting or quitting, the launcher retries for up to 5 s. Each attempt first tries to acquire the same lock; if the previous process has exited, the launcher becomes the new owner and starts normally. Otherwise it reads a complete record for the matching key and sends a show command to `control-<pid>.sock`. Disconnects, unavailable sockets and rejected handovers (including `quitting`) are retried within that deadline. A successful handover exits 0 and prints `Ultra 9000 is already running (pid N); showing its window.` to stderr; exhausting the deadline exits 1 with an explanation, never starting without the lock. An ordinary release launch also hands over to a live pre-lock v0.3.0 control socket, excluding every pid that owns a held `instance*.lock` (so newer debug/isolated instances are not legacy owners). v0.2.0 has no control socket and cannot be detected.

Debug builds use `instance-debug.lock` and do not hand over to the installed release app. For an isolated run of either build profile, set `ULTRA9000_INSTANCE=<name>`: 1–32 characters drawn from lowercase ASCII letters, digits, `_` and `-`; invalid names fail before GUI startup. This selects `instance-<name>.lock` and disables the pre-lock fallback. Use a unique name for native test harnesses, alongside isolated config/state/data directories (the name isolates the instance lock, not settings or session stores). `ultra9000 ctl` never takes an instance lock.

Startup fails rather than starting an unprotected instance if the runtime directory is not private. The error names its path, whether you own it, and its mode: it must be a non-symlink directory owned by you, with no group/other access (normally mode 0700). Set `XDG_RUNTIME_DIR` on Linux, or `TMPDIR` on macOS, to a private directory owned by you. If the app's runtime directory is abandoned or squatted, remove it only after ensuring no Ultra 9000 instance is using it; never delete a live lock file.

## Terminal file links

Clicking a local `file://` link in the terminal, including OMP's file links, validates an existing regular file and inspects a bounded content prefix off the UI thread. Markdown follows the document setting below; other files follow one of three routes:

- **Text/code → a new Neovim item**, selected immediately after the originating item in the same project, with that item's working folder (not the file's parent folder). This includes extensionless text, readable scripts even with executable permission, UTF-8 and BOM-marked UTF-16, and ANSI-coloured logs. `.svg` names instead use the system image handler; SVG-looking source/templates with other extensions remain text.
- **Launchers/executables → reveal in the file manager, never execute.** Recognized executable content, launcher names (including symlink targets), non-text script payloads and otherwise unrecognized executable data are revealed rather than passed to a default handler. Recognized documents/media/archives with non-launcher, non-script extensions still use the system opener even if the filesystem marks them executable; extensionless executable data is revealed.
- **Other non-text files → the system default opener** (`xdg-open` on Linux, `open` on macOS), including ordinary documents, archives and media.

On Linux, reveal requests FileManager1 `ShowItems` to select the file. Only a missing/unsupported service (`ServiceUnknown`, `UnknownMethod` or `NameHasNoOwner`) falls back to `xdg-open` on the **containing directory**, never the launcher. Other D-Bus errors, including timeouts, are reported without a second launch. macOS reveal uses `open -R`. Linux requires **`xdg-utils`** at runtime (included in the Arch packages). Opener spawn and missing-handler failures show an error; in generic desktop mode `xdg-open` may wait until the viewer exits and does not reliably report viewer failures.

The originating item keeps running; links in retained exited output also work. Missing files, directory links, special files, remote file hosts and malformed file URLs show an error instead of launching anything. Empty/`localhost` authorities and this machine's hostname identify local files. URL-encoded spaces, Unicode and reserved filename characters are decoded as literal path data. Line/column query parameters and fragments are not used for cursor positioning.

Neovim uses the normal `[tools.neovim]` command and arguments and must be available on the login-shell PATH. Web links still use the system opener. Custom terminal hint commands and copy/select actions remain unchanged; this routing applies to the default `open` / `xdg-open` hint action with no configured arguments. No global OMP, Neovim or Alacritty settings are changed.

## Documents

Markdown (`.md` / `.markdown`, case-insensitive) opens in a live, read-only document surface by default. Each file has a row after its project's process rows; opening it again reuses that row (selection changes only when requested). Files are watched, atomic saves do not flash a missing state, and closing never asks or changes the file. Documents have no process status and never affect tray, menu-bar or shell-plugin counts. The Open palette lists recent project documents; **Documents in this project** focuses that section. `e` (or the New Neovim shortcut) opens the current file in Neovim, reusing an editor already started for it. With `projects.roots` configured, Markdown links outside those roots retain the Neovim route.

```toml
[file_links]
markdown = "view" # "view" (default) or "neovim"; hot-reloaded
```

This setting is also available in Settings. Choose `neovim` to retain terminal links' editor route. The reader supports files up to 8 MiB; local images are bounded and confined to the document directory subtree. Remote images load automatically over HTTPS with no referrer. Open a file from a script without stealing selection:

```sh
ultra9000 ctl open-document ./docs/report.md
ultra9000 ctl open-document ./docs/report.md --select --follow
```

The CLI prints the item ID. `--select` switches to it; `--follow` asks the reader to follow new content. Last-set resume reopens documents from disk alongside resumable agents.

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
| Documents in this project (`documents`) | Ctrl+Shift+D | Cmd+D |
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

On macOS, left-clicking the menu-bar item toggles a keyboard-focused popover without
activating the app or bringing its main window forward. Right-click or Ctrl-click opens
the existing native tray menu. Esc, click-away, changing Spaces, or hiding the menu-bar
item dismisses the popover without ending any item. The panel is anchored below the status
button on its own display and its content height is clamped to the available screen area.
The static capsule shows the U_ mark (with a dot for unread turns), an orange needs-you
pill, red errors, a purple working arc and an on-job ring. Zero segments are omitted;
counts above 99 display `99+`, while accessibility labels retain exact totals. Colours
follow the menu bar's own light/dark appearance, and the capsule never animates. Busy
shells, successful agent exits, shell exits and seen turns do not add status segments;
failed launches and unsuccessful/signalled agent exits add red errors, using the same
classification as the control snapshot and Noctalia capsule. In the popover, h toggles
the main window without taking focus; n/t/e create rows without closing the panel;
selecting a row, j or o brings the main window forward. Popover focus never marks a turn
seen: only main-window keyboard focus does. Linux keeps its existing tray behaviour.
If native popover setup is unsupported or fails, the app keeps running and left-click opens
the existing menu instead; the failed window and its event registrations are removed.

### Control link

The running app listens on a private local socket (`control-<pid>.sock` next to the OMP status link: `$XDG_RUNTIME_DIR/ultra-9000/` on Linux, the per-user temp directory on macOS) that a shell plugin such as Noctalia's `muty/ultra9000` uses through the app's own command-line client, which never starts the app:

```sh
ultra9000 ctl watch [--client NAME]       # prints the app's lines (newline-delimited JSON) until stdout closes
ultra9000 ctl select_item '{"itemId":"item-3"}'
ultra9000 ctl jump_to_waiting
ultra9000 ctl set_window_visible '{"visible":"toggle"}'
ultra9000 ctl set_window_visible '{"visible":true,"activationToken":"<XDG_ACTIVATION_TOKEN>","desktopStartupId":"<DESKTOP_STARTUP_ID>"}' # activation fields optional
ultra9000 ctl new_item '{"projectId":"/abs/project","kind":"shell"}'   # kind: agent | shell | neovim; projectId and tool optional
ultra9000 ctl new_item '{"kind":"agent"}' # omitted projectId uses the launch-default directory/tool
ultra9000 ctl open_palette '{"projectId":"/abs/project","mode":"new"}' # projectId optional; mode open (default) | new
ultra9000 ctl close_item '{"itemId":"item-3","confirmed":false}'
ultra9000 ctl request_quit
```

`new_item {projectId?, kind, tool?}` launches in a listed project's root, or in the app's
launch-default directory when `projectId` is omitted; neither path shows the main window.
`open_palette {projectId?, mode?}` opens the main window's palette in `open` mode by default;
`mode: "new"` opens the project's new-item picker, as its sidebar + does.

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
