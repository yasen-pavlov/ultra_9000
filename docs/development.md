# Development

See also [CONTRIBUTING.md](../CONTRIBUTING.md) for the branch/PR workflow and [releasing.md](releasing.md) for versioning and releases.

## Requirements

Common:

- A current stable Rust toolchain with Cargo, at least the `rust-version` declared in `src-tauri/Cargo.toml` (CI checks that MSRV). The dependency lock has been built with Rust 1.94.1 (macOS) and 1.99.0 (Linux).
- [Bun](https://bun.sh/) (the source has been checked with Bun 1.3 and 1.4).
- Recommended terminal font: **JetBrainsMono Nerd Font**, matching the app's 15pt fidelity baseline. Install it separately; fonts are not bundled.
- Optional launch targets: install `omp`, `nvim`, `claude`, `codex` and/or `pi` separately and make them available in the PATH of your interactive login shell. The shell profile works independently of these tools. Provider authentication and Neovim configuration are your workstation's own, not included here.

**macOS** (primary target): Apple Silicon is the tested architecture; the bundle declares macOS 12 as its minimum, which (like Intel Macs) has not been verified. Xcode Command Line Tools: `xcode-select --install`.

**Linux** (Wayland only; developed on CachyOS with Hyprland 0.56): the Tauri GTK stack (`webkit2gtk-4.1`, `gtk3`, `librsvg`), Mesa EGL and `libxkbcommon`. Package names per distribution are in the [README](../README.md#other-linux-distributions). The tray uses pure-Rust `ksni` over the session D-Bus and needs a desktop StatusNotifierWatcher (for example Noctalia/Quickshell), not libayatana-appindicator/libappindicator. GDK must use its Wayland backend; under X11/XWayland the terminal host refuses to start with an explicit error. Integer display scale only; fractional scaling and mixed-DPI monitors are not supported.

The OMP/Neovim launch path currently supports POSIX login shells (sh, bash, zsh, ksh, dash). Other shells receive an actionable error. It resolves installed tools rather than hardcoding machine paths.

## Clone and run

Clone the repository. From the clone root:

```sh
bun install --frozen-lockfile --registry https://registry.npmjs.org
bun run check
bun run tauri dev
```

Dependency tarballs in `bun.lock` use public npm. No private package registry or copied authentication file is required. Rust registry dependencies come from crates.io; vendored Alacritty paths are relative to this repository.

Build a standalone debug application, with no development server required at runtime:

```sh
bun run tauri build --debug --bundles app      # macOS
open "src-tauri/target/debug/bundle/macos/Ultra 9000.app"

bun run tauri build --debug --no-bundle        # Linux
./src-tauri/target/debug/ultra9000
```

The Vite development server listens on `127.0.0.1:1420`. The app has its own bundle identity and does not modify the stock Alacritty application or global Alacritty/Neovim settings.

For an installable Arch package from your checkout: `cd packaging/arch && makepkg -si`.

## Checks

Run these before pushing; CI runs the same lint, format, type, test and build checks (plus an MSRV build and `cargo deny`), aggregated in the required `CI ok` status.

```sh
bun run check                 # svelte-check
bun run lint                  # ESLint (incl. .svelte)
bun run format:check          # Prettier; `bun run format` rewrites
bun run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Focused tests:

```sh
cargo test --manifest-path src-tauri/Cargo.toml launch::tests -- --nocapture
cargo test --manifest-path vendor/alacritty_terminal/Cargo.toml --target-dir src-tauri/target
cargo test --manifest-path src-tauri/Cargo.toml -p alacritty_embed --lib
cargo test --manifest-path vendor/vte-graphics/Cargo.toml --features ansi --target-dir src-tauri/target
cargo test --manifest-path vendor/alacritty_terminal/Cargo.toml --target-dir src-tauri/target drop_stops_foreground_job_ignoring_hangup -- --nocapture  # macOS only
```

The vendored crates are path dependencies, not members of the app workspace; use their explicit manifest paths. Do not reformat or lint-fix vendored code (`cargo fmt`/`clippy` above target the app crate). The `alacritty_terminal` suite includes the Sixel fixtures, OSC 1337 and kitty graphics tests. The build is warning-free on Linux and the macOS cross-check; keep it that way without blanket `#[allow]`.

### GUI integration tests

`src-tauri/tests/gui_linux.rs` and `gui_macos.rs` run production code on the real Tauri runtime on the process's main thread (they bring their own `main`; libtest never runs tests there). Without a display they print a skip and pass; `U9K_REQUIRE_GUI_TESTS=1`, set in CI, turns the skip into a failure. Neither starts a session.

On Linux the test runs the app's startup (main window, terminal host, shortcut router, tray), checks the window's GTK state (title, undecorated, visible, realized, mapped, Wayland backend), that navigation away from the app's UI is blocked, that focus-out and focus-in reach the app's run-loop handler and the window's focus state, hides the window with a close request and shows it again, hides it with Ctrl+Shift+H through the shortcut router, cancels and accepts the folder chooser, fails on any GLib/GTK warning, and finally destroys the window to check the app outlives it. Point it at a throwaway nested compositor, never your desktop session, with isolated XDG directories and a private session bus (the app registers its tray there). `--features tauri/custom-protocol`, as in release builds and CI, loads the bundled UI from `dist/` (run `bun run build` first); without it the window points at the dev server:

```sh
env -u DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE WAYLAND_DISPLAY=<nested socket> \
  XDG_CONFIG_HOME=<dir>/config XDG_STATE_HOME=<dir>/state XDG_DATA_HOME=<dir>/data \
  dbus-run-session -- cargo test --manifest-path src-tauri/Cargo.toml --features tauri/custom-protocol --test gui_linux
```

CI uses a headless sway instead: `WLR_BACKENDS=headless WLR_RENDERER=pixman sway --config /dev/null` in a fresh `XDG_RUNTIME_DIR`, then `WAYLAND_DISPLAY=wayland-1`. Headless Weston does not work: it has no `wl_seat`, and GTK logs criticals without one.

On macOS, `cargo test --manifest-path src-tauri/Cargo.toml --test gui_macos` in a logged-in session runs the same startup and checks the window's AppKit state (title; titled with a transparent full-size titlebar, hidden title and hidden traffic lights, so AppKit rounds the corners; shadow; content filling the window; visible; 640×400 minimum size), the terminal host, the application menu, the terminate hook and the UTF-8 locale. It hides the window with a close request and restores it through the application delegate's Dock reopen, then opens the folder picker the way production does (inside a main-queue block), checks it is the visible modal window and aborts it from a run-loop timer. A watchdog thread fails the test after 4 minutes instead of letting a stuck modal hang CI; every CI job also has a `timeout-minutes`.

Coverage of the app crate, as CI measures it (production lines only; on Linux with the GUI test's environment and feature above, or the GUI test is skipped and its lines count as uncovered):

```sh
cargo llvm-cov --manifest-path src-tauri/Cargo.toml --summary-only --ignore-filename-regex '/(tests|test_support)\.rs$|/src-tauri/tests/'
```

**Build output is not proof of native rendering/input acceptance.** Exercise the actual native app. GUI checks run in a throwaway nested Hyprland (`Hyprland --config <throwaway>.lua`, driven with `wtype`/`grim` on that compositor's socket only) with isolated `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_DATA_HOME` and a `--no-session` test agent tool. Use test-owned sessions when checking lifecycle behavior; never terminate unrelated work, and never let test prompts reach a real agent or resume a real saved conversation. Build output fills a 16 GB `/tmp`; keep extra target dirs under `~/.cache`.

### macOS cross-check from Linux

Compile check only (no linking; needs `clang` and `rustup target add aarch64-apple-darwin`):

```sh
CC_aarch64_apple_darwin=clang CFLAGS_aarch64_apple_darwin="-target arm64-apple-macos11" \
  cargo check --manifest-path src-tauri/Cargo.toml --target aarch64-apple-darwin --target-dir "$HOME/.cache/u9k-mac-check"
```

Native macOS runtime must be re-verified on a Mac after shared embed/vendor changes. Accessibility and Screen Recording permissions are needed only for UI automation/capture used during verification, not for ordinary app use. Configure permissions on the destination machine if using such automation.

## Platform notes

**macOS:** one NSWindow with system rounded corners, border and shadow but no visible title bar: it is titled, with a transparent full-size titlebar and its title and traffic lights hidden (like Alacritty's `decorations = "Buttonless"`), because AppKit rounds and clips only titled windows. Terminals are AppKit `NSView`s above the WKWebView. App shortcuts use Cmd and are matched by physical key code.

**Linux:** one undecorated GTK window. Each terminal has an input-only GTK widget in a `GtkOverlay` above the WebKitGTK view and its own `wl_surface`, mapped as a Wayland subsurface over the terminal rectangle and drawn by Alacritty's EGL renderer. There is no in-window menubar. App shortcuts use Ctrl+Shift and are matched by physical key, so they work under any layout (EurKEY, Bulgarian phonetic). Examples: Space palette, N/T/E new agent/terminal/Neovim, W close, J jump to what's waiting, 1–9 select, Up/Down previous/next, `,` settings, `/` all shortcuts, H hide, Q quit. The full map is in [configuration](configuration.md#keyboard-shortcuts). Alacritty's own Linux bindings (Ctrl+Shift+C/V/F/B, vi mode on Ctrl+Shift+[) apply in the terminal. The StatusNotifierItem tray (ksni) shows up to three app-owned running items, prioritized by needs-you and unread turns, a remainder link to the palette, Show/Hide, the needs-you notification setting and Quit. Left-click shows/focuses the window; right-click opens the menu; an attention dot marks needs-you. Closing the window from the compositor hides it.

## Terminal graphics

The vendored Alacritty includes ayosec's graphics patch (Sixel) and a port of ayosec's experimental iTerm2 inline-image support (OSC 1337 `File=…;inline=1:<base64>`; PNG, JPEG, GIF first frame, BMP, TIFF; `width`/`height` in cells, `px`, `%` or `auto`). Try `chafa -f sixels image.png`, `chafa -f iterm image.png`, `img2sixel image.png` or `magick image.png sixel:-`. Not supported: multipart OSC 1337 transfers, animation and WebP. OSC 1337 images are decoded synchronously on the PTY thread while it holds the terminal lock; for the largest accepted images (about 40 MP) a visible session's redraw, and with it the app's main loop, can stall for roughly 0.3 s. See [`vendor/PROVENANCE.md`](../vendor/PROVENANCE.md).

The kitty graphics protocol is implemented for directly transmitted PNG and raw RGB/RGBA data: chunked transfers, queries, direct placements and Unicode-placeholder (`U=1`) virtual placements, and deletes. File/shared-memory transfers, compression, animation and z-index layering are not supported. Every session gets `PI_FORCE_IMAGE_PROTOCOL=kitty` and `PI_KITTY_PLACEHOLDERS=1` (unless already set), so OMP uses kitty placeholders and shows real thumbnails for pasted images; `chafa -f kitty` works too. Scope: [`vendor/PROVENANCE.md`](../vendor/PROVENANCE.md).

## Lifecycle safety

Every row is a running process. Close ends that process and removes the row. For an agent, the conversation stays in its tool's store and can be resumed. Close asks first only for a working OMP agent, a shell with a foreground job, or Neovim. Hiding or closing the app window keeps everything running. Quit lists everything it will end, then offers "Resume last set" at the next start. Agent state (working, needs you, your turn) comes only from OMP's own terminal signals: the OSC 0 title and OSC 777 events enabled per launch with `WARP_CLI_AGENT_PROTOCOL_VERSION=1`. It is never inferred from output. Agent stores and global tool configuration are never written.

No daemon preserves processes after full quit, crash, logout or reboot. Cleanup manages the shell and foreground process groups, not deliberately detached/disowned/nohup processes.

## Source map

- `src/`: Svelte sidebar, palette, sheets, native command bindings and geometry/focus sequencing.
- `src-tauri/`: app startup and run-loop events (`lib.rs`; `main.rs` is the `ultra9000` executable), native item/project model (`workspace.rs`), commands, lifecycle, launch, settings files (`config/`), read-only agent store discovery (`discovery/`), OMP status (`status.rs`), shortcut keymap (`shortcuts.rs`), platform layers (`macos.rs`, `linux.rs`) and the GUI integration tests (`tests/`).
- `vendor/`: pinned open-source Alacritty components with the graphics patch and the macOS/Linux embedding adaptations; see [provenance and licenses](../vendor/PROVENANCE.md).
- `packaging/arch/`: in-tree Arch Linux PKGBUILD (the release's package). `packaging/aur/`: the AUR packages' PKGBUILD templates and `publish.sh` ([releasing](releasing.md#aur-packages)).
- `.github/`: CI, release and dependency-update workflows, issue/PR templates and the repository rulesets.
- `assets/icons/`: SVG masters for the app, tray, tool and provider icons, and the third-party mark sources with their licenses (`brand-sources/`).
- `scripts/`: `icons.py` regenerates the committed icon PNGs/ICNS from `assets/icons/`; `set-version.ts` stamps release versions.
- `patches/README.md`: optional, separate OMP footer change and how to apply it. It is not required to build Ultra 9000.

## Repository boundary

Machine-specific agent configuration, credentials, local session data, verification screenshots, transcripts, evaluation archives, dependency caches, build artifacts and `docs/local/` are deliberately excluded. A new clone is sufficient for app development; no files from the original workstation's home directory are required. Version fields stay at the `0.0.0` placeholder: release builds stamp the tag's version (`docs/releasing.md`). The README screenshot (`docs/screenshot.png`) is staged with fictional projects, conversations and data.
