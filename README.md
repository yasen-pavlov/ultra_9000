<div align="center">

<img src="src-tauri/icons/128x128.png" width="96" height="96" alt="Ultra 9000 icon">

# Ultra 9000

**A keyboard-first agent harness with real native Alacritty terminals.**

[![CI](https://github.com/yasen-pavlov/ultra_9000/actions/workflows/ci.yml/badge.svg)](https://github.com/yasen-pavlov/ultra_9000/actions/workflows/ci.yml)
[![Quality Gate Status](https://sonarcloud.io/api/project_badges/measure?project=yasen-pavlov_ultra_9000&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=yasen-pavlov_ultra_9000)
[![Coverage](https://sonarcloud.io/api/project_badges/measure?project=yasen-pavlov_ultra_9000&metric=coverage)](https://sonarcloud.io/summary/new_code?id=yasen-pavlov_ultra_9000)
[![Release](https://img.shields.io/github/v/release/yasen-pavlov/ultra_9000)](https://github.com/yasen-pavlov/ultra_9000/releases/latest)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

</div>

## What it is

Ultra 9000 is a Tauri + Svelte desktop app that wraps **actual native Alacritty rendering and input** (no xterm.js, no browser terminal emulation) in a restrained project-centric interface. Agents, shells and Neovim run in app-owned PTYs, grouped by project in a sidebar.

- **Projects → live items.** Every sidebar row is one live process: an agent (OMP, Claude Code, Codex, pi, or any tool configured in `[tools.*]`), a shell or Neovim, grouped by git project.
- **Native terminals.** Vendored Alacritty drawn as AppKit views on macOS and Wayland subsurfaces on Linux, above only the terminal rectangle.
- **Resume saved conversations** from OMP, Claude Code, Codex and pi, discovered read-only from each tool's own session store.
- **Agent status from the agent's own signals.** Working, needs you and your turn come from OMP's terminal title and OSC 777 events, never guessed from output.
- **Inline images:** Sixel, iTerm2 OSC 1337 inline files and the kitty graphics protocol, including Unicode placeholders (OMP's image thumbnails).
- **Git-friendly settings.** Plain TOML files are the source of truth, hot-reloaded; the settings sheet edits them in place and keeps comments.
- **Keyboard-first**, with first-class mouse use; shortcuts match physical keys, so they work under any layout.
- **Tray and notifications.** Hiding keeps every item running; the tray shows what needs you.

## Platform support

| Platform | Status | Distribution |
| --- | --- | --- |
| macOS 12+ on Apple Silicon (arm64) | Supported; Apple Silicon is the tested architecture (macOS 12 minimum declared, not verified) | Signed and notarized `.dmg` |
| Linux, Wayland, integer display scale | Supported; developed on CachyOS with Hyprland | Arch package (`.pkg.tar.zst`) / build from source |
| Intel Macs, fractional scaling, mixed-DPI monitors | Not supported or untested | — |
| X11 / XWayland | Not supported (the terminal host refuses to start with an explicit error) | — |

## Installation

Download the latest build from [GitHub Releases](https://github.com/yasen-pavlov/ultra_9000/releases/latest). `SHA256SUMS.txt` lists the checksum of every asset.

### macOS

Download `ultra-9000_<version>_aarch64.dmg`, open it and drag **Ultra 9000** to Applications. The app is signed with a Developer ID and notarized by Apple.

### Arch Linux / CachyOS

From the AUR, either the prebuilt release package or a build from source:

```sh
yay -S ultra-9000-bin   # the release's package, installs in seconds
yay -S ultra-9000       # builds the release tag from source (Rust and Bun)
```

Or install the release package directly:

```sh
sudo pacman -U ultra-9000-*-x86_64.pkg.tar.zst
```

Or build the package from a clone:

```sh
git clone https://github.com/yasen-pavlov/ultra_9000.git
cd ultra_9000/packaging/arch && makepkg -si
```

### Other Linux distributions

Build from source (see [development](docs/development.md#clone-and-run)) with Rust, [Bun](https://bun.sh/) and the GTK/WebKitGTK stack:

```sh
# Debian / Ubuntu
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev \
  librsvg2-dev libssl-dev libdbus-1-dev libxkbcommon-dev libegl-dev

# Fedora
sudo dnf install gcc gcc-c++ pkgconf-pkg-config webkit2gtk4.1-devel gtk3-devel \
  librsvg2-devel openssl-devel dbus-devel libxkbcommon-devel mesa-libEGL-devel
```

Then `bun run tauri build --no-bundle` and run `src-tauri/target/release/ultra9000`.

### Runtime requirements

- **Linux tray:** a desktop StatusNotifierWatcher (for example Noctalia/Quickshell). libappindicator is not used.
- **Font:** JetBrainsMono Nerd Font is recommended (15pt baseline); fonts are not bundled.
- **Optional tools:** `omp`, `nvim`, `claude`, `codex` and `pi` on the PATH of your interactive login shell (POSIX shells: sh, bash, zsh, ksh, dash). Shells work without any of them.

## Usage

Start the app and press the palette shortcut to switch to a running item, resume a saved conversation or start something new. Linux uses **Ctrl+Shift**, macOS uses **Cmd**:

| Action | Linux | macOS |
| --- | --- | --- |
| Palette (switch / resume / start) | Ctrl+Shift+Space | Cmd+O |
| Palette scoped to the current project | Ctrl+Shift+R | Cmd+R |
| New agent / terminal / Neovim here | Ctrl+Shift+N / T / E | Cmd+N / T / E |
| Close item (again = confirm) | Ctrl+Shift+W | Cmd+W |
| Jump to what's waiting | Ctrl+Shift+J | Cmd+J |
| Item 1…9 | Ctrl+Shift+1…9 | Cmd+1…9 |
| Previous / next item | Ctrl+Shift+Up / Down | Cmd+Shift+[ / ] |
| Previous / next project | Ctrl+Shift+PageUp / PageDown | Cmd+Option+[ / ] |
| Focus / collapse sidebar | Ctrl+Shift+L / S | Cmd+L / Ctrl+Cmd+S |
| Settings / all shortcuts | Ctrl+Shift+, / / | Cmd+, / / |
| Hide / quit | Ctrl+Shift+H / Q | Cmd+H / Q |

Alacritty's own bindings (copy/paste, search, vi mode on Ctrl+Shift+[) apply in the terminal. The full map, the `keys.toml` action names and the keys the app never captures are in [docs/configuration.md](docs/configuration.md#keyboard-shortcuts).

Hiding or closing the window keeps everything running. Closing an item ends its process; an agent's conversation stays resumable. Quit lists everything it will end and offers "Resume last set" at the next start.

## Configuration

Settings live in `~/.config/ultra-9000/` (`ultra.toml`, `terminal.toml`, `keys.toml`, `machines/<hostname>.toml`) and are meant to be kept in git. Your global Alacritty, Neovim and agent configuration is never read or written. See [docs/configuration.md](docs/configuration.md) for files, icons and provider marks.

## Development

```sh
bun install --frozen-lockfile
bun run tauri dev
```

Requirements, checks, platform internals and the source map are in [docs/development.md](docs/development.md). Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## Releases

Releases are signed `vX.Y.Z` tags on `main`; the tag is the version. Pushing one builds the signed, notarized macOS app and the Arch package and publishes them with release notes generated from the Conventional Commit history. See [docs/releasing.md](docs/releasing.md).

## Documentation

- [Configuration](docs/configuration.md): settings files, keyboard shortcuts, icons and provider marks.
- [Development](docs/development.md): requirements, checks, platform internals and the source map.
- [Releasing](docs/releasing.md): versioning, the release pipeline, CI and repository setup.
- [Vendor provenance](vendor/PROVENANCE.md): pinned upstream sources and local changes.
- [Security policy](SECURITY.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

The vendored Alacritty components under `vendor/` keep their upstream Apache-2.0/MIT licenses; see [vendor/PROVENANCE.md](vendor/PROVENANCE.md). Bundled tool and provider marks are attributed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md); check the owners' brand guidelines and obtain the pi project's permission for its logo before any public redistribution.
