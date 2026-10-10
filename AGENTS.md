# Ultra 9000 project context

Read `docs/local/handoff.md`, `docs/local/native-ui-contract.md` and `docs/local/design/ux-redesign/design.md` before planning changes. These maintainer notes are gitignored and exist only in the maintainer's checkout; in a fresh clone, start from `README.md` and `docs/`. Ultra 9000 is an agent harness: projects → live items (agents, shells, Neovim) with saved agent conversations resumable from the palette. The approved design (2026-10-03) is implemented per contract v2 on `main` (merged 2026-10-04). `docs/local/ux-handover.md` is the brief the design came from.

## Product invariants

- Tauri/Svelte application UI around actual native Alacritty rendering/input, not xterm.js, a simulated terminal, screenshot streaming or an external overlay window.
- macOS and Linux (GTK 3/WebKitGTK on Wayland, user-approved 2026-10-03) are both targets; changes must keep both building (macOS cross-check from Linux is in `docs/development.md`). X11, fractional scaling and mixed-DPI are out of scope. Native macOS runtime must be re-verified on a Mac after shared embed/vendor changes.
- The vendored terminal carries ayosec's Sixel graphics patch, an OSC 1337 (iTerm2 inline image) port, kitty graphics, BEL host-event forwarding, live config reload and physical-key app routing; keep `vendor/PROVENANCE.md` exact when touching them.
- Current preferences: no title bar, traffic lights or top gap; restrained, keyboard-first UI with first-class mouse use. Approved navigation: a vertical sidebar of running items grouped by project (no bottom tabs or icon rail); saved sessions via the palette or a project's +.
- Hide/close-to-tray retains every live item. Explicit quit shows the quit sheet and stops all app-owned processes. Every row is one live process: close ends it, and an agent's saved conversation stays resumable. No full-quit/crash persistence daemon is intended.
- Read-only discovery of OMP session stores (and, lighter, Claude Code, Codex and pi) and use of OMP's own emitted status signals are in scope (user-approved 2026-10-03). Never write to those stores, infer agent status from output/CPU, or modify global Alacritty, Neovim, OMP or other agent configuration; launch-scoped environment variables or extensions are allowed.
- App settings live in git-friendly files as the source of truth; any settings UI writes those files.

## Engineering workflow

- Use separate implementation and independent-review roles for substantial changes. Configure available models on the current workstation; no provider credentials or model-routing configuration are required to build this app.
- When using OMP workers, use its native task/workpool and coordination facilities, not separate CLI worker sessions or custom orchestration.
- Define interfaces and disjoint file ownership before concurrent work; at most three workers. One owner integrates native changes. Do not run validation while peers mutate shared inputs.
- After the mutation barrier, build and exercise the actual native app. A successful build or browser preview is not native acceptance.
- Preserve upstream source provenance and licenses; read `vendor/PROVENANCE.md` before adapting vendored source. Do not blanket-suppress warnings; the build is warning-free on Linux and the macOS cross-check and stays that way.
- Keep tests focused on user-visible behavior, boundaries and ordering. Record verification limits honestly.
- Edit this checkout directly on a topic branch. If a `bun run tauri dev` app is running from it, restarting it (and any session inside it) on Rust changes is fine; it is only a test instance (user decision, 2026-10-04). Worktrees are for parallel workers.
- Changes reach `main` only through squash-merged pull requests; never push to `main`. PR titles are Conventional Commits (`feat`, `fix`, `perf`, `refactor`, `docs`, `design`, `style`, `test`, `build`, `ci`, `chore`, `revert`; `!` for breaking) because they become the commit subject and drive the release notes. The `CI ok` and `PR title` checks must pass. Every commit and release tag is GPG-signed; never disable signing. Releases are signed tags `vX.Y.Z` on `main`; the tag is the version (`docs/releasing.md`). See `CONTRIBUTING.md`.
- Describe the plan to the user before changing anything beyond the agreed scope, and wait for agreement.
- GUI checks run in a throwaway nested Hyprland with isolated `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_DATA_HOME` and a `--no-session` test agent tool. Never let test prompts reach a real agent or resume a real saved conversation.

## Portable repository boundary

The repository contains application source, dependency locks, vendored open-source dependencies/licenses, the project's `MIT OR Apache-2.0` license files, CI/release/packaging configuration, the icon masters in `assets/icons/` and user/contributor documentation. Version fields stay at the `0.0.0` placeholder; release builds stamp the tag's version. The optional OMP footer patch is separate from the app and described in `patches/README.md`.

Design records, the native/UI contract, handoff and handover notes live in the gitignored `docs/local/` (`docs/design` is a local symlink into it, so the Claude Design tooling keeps its path). Local agent configuration, credentials, private registries, verification screenshots, transcripts, databases, model-evaluation artifacts and build products are excluded too; the only committed screenshot is the README's `docs/screenshot.png`, staged with fictional data. Never force-add those files. Do not copy workstation-wide settings into the repository. Install and fresh-clone instructions are in `README.md` and `docs/development.md`; `docs/local/handoff.md` is the maintainer's continuation entry point.
