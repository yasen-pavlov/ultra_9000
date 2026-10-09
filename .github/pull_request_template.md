## Summary

<!-- What changes and why. The PR title must be a Conventional Commit: <type>[(scope)][!]: <description> -->

## Type

- [ ] feat
- [ ] fix
- [ ] perf / refactor
- [ ] docs / design
- [ ] build / ci / chore
- [ ] Breaking change (`!` in the title)

## Checks run

- [ ] `bun run check`, `bun run lint`, `bun run format:check`, `bun run build`
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, relevant `cargo test` suites
- [ ] macOS cross-check (`cargo check --target aarch64-apple-darwin`) for shared native/vendor changes
- [ ] Native verification in a nested Hyprland with isolated XDG directories (UI/native changes)
- [ ] macOS runtime check on a Mac (or noted as not run)

<!-- List what was verified and what was not. -->

## Docs

- [ ] Screenshot or recording of the real app attached (UI changes)
- [ ] `vendor/PROVENANCE.md` updated for vendored changes
- [ ] User-facing docs (README, `docs/configuration.md`, `docs/development.md`) updated
