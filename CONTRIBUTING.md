# Contributing

Thanks for helping with Ultra 9000. Read [AGENTS.md](AGENTS.md) for the product invariants and [docs/development.md](docs/development.md) for setup.

## Branches and pull requests

- `main` is protected by a branch ruleset (`.github/rulesets/main.json`): no direct or force pushes; changes land through pull requests.
- Work on a topic branch (`feat/…`, `fix/…`, `chore/…`) and open a pull request against `main`.
- Pull requests are **squash-merged**, history stays linear and the branch is deleted on merge.
- The required status check is **`CI ok`**, which aggregates every CI job.

## PR titles: Conventional Commits

The squash commit uses the PR title as its subject, and the release notes are generated from those subjects (`cliff.toml`). Titles must follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>[(scope)][!]: <description>
```

Allowed types: `feat`, `fix`, `perf`, `refactor`, `docs`, `design`, `style`, `test`, `build`, `ci`, `chore`, `revert`. Use `!` (or a `BREAKING CHANGE:` footer) for breaking changes. Examples: `feat(palette): resume pi sessions`, `fix(linux): keep subsurface aligned on resize`.

## Before pushing

```sh
bun run check
bun run lint
bun run format:check
bun run build
bun run test:coverage
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo llvm-cov --manifest-path src-tauri/Cargo.toml --summary-only --ignore-filename-regex '/(tests|test_support)\.rs$|/src-tauri/tests/'
```

Keep the build warning-free on Linux and the macOS cross-check; fix warnings rather than suppressing them. For native or UI changes, also exercise the actual app in a throwaway nested Hyprland with isolated XDG directories (see [development](docs/development.md#checks)); build output is not native acceptance. Record what was and was not verified in the PR.

## Tests and coverage

SonarQube Cloud's quality gate requires 80% coverage on new code, measured on production lines only, with no coverage exclusions. Test user-visible behavior, boundaries and error paths, not wiring.

- Rust tests go in a dedicated module file next to the code: `foo.rs` declares `#[cfg(test)] mod tests;` and the tests live in `foo/tests.rs` (shared fakes in `test_support.rs`). Never write test code inline in a production file; it would be counted as covered source.
- Platform glue that only runs on the real runtime and the main thread (window creation, GTK/AppKit hosts, dialogs) is covered by the GUI integration tests in `src-tauri/tests/` (`gui_linux.rs`, `gui_macos.rs`; `harness = false`, through the library's `testing` module). Run `gui_linux` against a nested compositor, never your desktop session ([development](docs/development.md#gui-integration-tests)); without a display it skips, and CI sets `U9K_REQUIRE_GUI_TESTS=1` so it cannot.
- Frontend tests are `src/**/*.test.ts` (Vitest, jsdom, Tauri IPC mocked with `@tauri-apps/api/mocks`); shared helpers go in `src/test/`.

## UI and design changes

For anything beyond a small fix, open an issue describing the change (screens, states, keyboard behavior) before implementing it, so the design can be agreed first. UI changes need a screenshot or short recording of the real app in the pull request, and must keep keyboard-first use and the existing visual system (`src/app.css`) intact.

## Vendored code

`vendor/` holds pinned upstream Alacritty components. Read [vendor/PROVENANCE.md](vendor/PROVENANCE.md) before touching them, keep upstream style (do not reformat or lint-fix), preserve their licenses, and record every change there exactly.

## Commit signing

Every commit and release tag is GPG-signed (`git commit -S`, `git tag -s`); release tags must carry a signature GitHub verifies.

## License

By contributing you agree that your contributions are dual licensed under `MIT OR Apache-2.0`, as described in the [README](README.md#license).
