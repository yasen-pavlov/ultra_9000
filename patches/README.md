# Optional OMP footer patch

Ultra 9000 builds independently of OMP source. Use an official OMP installation by default. The previously used local OMP fork was retired; this directory preserves only the project-related footer work as an **optional historical continuation artifact**, not a requirement to recreate that fork.

## Scope

`oh-my-pi-bottom-anchor.patch` changes short OMP composer frames to stay at the bottom of the terminal, including startup, resize, clear/replay and the first frame after a fullscreen overlay. Layout-only padding is not transcript history, and click spans follow the padding.

The TUI exposes `getFrameViewportTop()`, the authoritative normal-paint origin, separately from hit-test coordinates. Generic provider top alignment remains unchanged.

The patch contains exactly seven paths:

- `packages/tui/src/prompt/composer.ts`
- `packages/tui/src/tui.ts`
- `packages/coding-agent/test/composer-bottom-anchor.test.ts` (new)
- `packages/tui/test/composer-click.test.ts`
- `packages/coding-agent/test/welcome-history-resize.test.ts`
- `packages/coding-agent/CHANGELOG.md`
- `packages/tui/CHANGELOG.md`

It excludes all unrelated model/provider/catalog work, personal configuration, development transcripts, backup archives and commit author metadata. It does not install or authenticate OMP.

## Public source and exact base

- Upstream: <https://github.com/can1357/oh-my-pi>
- Base: [`fdd58c98969d3eefcda9705bc563b7a55344d3bf`](https://github.com/can1357/oh-my-pi/commit/fdd58c98969d3eefcda9705bc563b7a55344d3bf) (18.4.2-era source).
- Upstream license: MIT; exact base license preserved in `LICENSE.oh-my-pi`.
- Change origin: local, model-assisted project work for the terminal footer request, reconstructed as a plain unified diff against the matching public source. Upstream later moved the composer from the coding-agent package into TUI; this patch uses the migrated paths above.

Do not apply blindly to a newer release. The pinned base is a reproducible reference, **not a recommendation to downgrade an installed tool**. Prefer porting the small change to a current release if you decide to maintain it.

## Apply in a separate checkout

From the Ultra 9000 clone, with a free adjacent `oh-my-pi` directory:

```sh
git clone https://github.com/can1357/oh-my-pi.git ../oh-my-pi
git -C ../oh-my-pi switch --detach fdd58c98969d3eefcda9705bc563b7a55344d3bf
git -C ../oh-my-pi switch -c ultra9000-bottom-anchor
git -C ../oh-my-pi apply --check ../ultra_9000/patches/oh-my-pi-bottom-anchor.patch
git -C ../oh-my-pi apply ../ultra_9000/patches/oh-my-pi-bottom-anchor.patch
```

The relative patch path assumes the app clone is named `ultra_9000`; adjust it if you choose another directory name. The patch does not modify your installed OMP until you separately follow that checkout's setup/install instructions.

## Verification boundaries

The earlier implementation, before the upstream package migration, passed 79 focused tests (681 assertions), type checks and independent source review. That result is historical evidence, **not a claim that the migrated patch's suite has been rerun against this base**.

For this transfer, the final seven-file patch is checked for clean application to exact public base files and for excluding unrelated hunks. Re-run the base repository's focused tests and type checks before using a patched build; read its own contributor instructions first. The key regression cases are in `composer-bottom-anchor.test.ts`, `composer-click.test.ts` and `welcome-history-resize.test.ts`.

The last native acceptance checkpoint still lacked a visual check of the corrected footer in a new OMP session. Existing OMP processes do not hot-reload source changes. Shrinking a bottom-pinned terminal can legitimately move older content into scrollback; tests must protect exact-once retained history, not require every old row to remain visible.
