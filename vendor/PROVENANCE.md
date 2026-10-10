# Vendored source provenance

All Alacritty sources come from https://github.com/alacritty/alacritty at commit
`def47a5dd511ed20607eb5d9e5e7140afd16b31d` (alacritty 0.16.0-dev).
Licenses: `LICENSE-APACHE` (Apache-2.0) and
`LICENSE-MIT`, copied from the repository root. `alacritty` and `alacritty_terminal` are
Apache-2.0; `alacritty_config` and `alacritty_config_derive` are MIT OR Apache-2.0.

## Core crates

| Directory | Upstream path | Notes |
|---|---|---|
| `alacritty_terminal/` | `alacritty_terminal/` | `tests/` (46 MB reference fixtures) omitted, except the small `tests/sixel/` fixtures of the graphics patch; `tests/iterm2/` is new (see "Graphics"). Explicit `rustix/std` is enabled in Cargo.toml. `src/tty/unix.rs` adds reap-safe, bounded foreground/shell-group shutdown and owned-child regressions (macOS and Linux), including Linux leaderless pipelines and starttime validation; see the file header and "Agent harness embed boundary" below. Other changes are listed under "Graphics" and the embed boundary. |
| `alacritty_config/` | `alacritty_config/` | License symlinks replaced by copies. embed: `src/lib.rs` adds scoped, thread-local strict deserialization with panic-safe restoration; ordinary upstream loaders keep their forgiving behavior. |
| `alacritty_config_derive/` | `alacritty_config_derive/` | License symlinks replaced by copies. embed: `src/config_deserialize/de_struct.rs` propagates invalid field and flattened-field errors only in the scoped strict mode. Unknown-key warnings and upstream default mode remain unchanged. |
| `vte-graphics/` | crates.io `vte-graphics` 0.15.0 (https://github.com/ayosec/vte-graphics, commit `22c7c29c0303398baa71db8798473ee704f9717c`; Apache-2.0 OR MIT, licence files included) | Vendored for kitty graphics: `src/lib.rs` adds an `ApcString` parser state that passes APC strings (`ESC _ … ST`) to new `Perform::apc_start`/`apc_put`/`apc_end` hooks in batched slices (SOS and PM are still discarded) plus parser tests; `src/ansi.rs` adds the matching `Handler` hooks. `Cargo.toml` is the crate's original manifest (`Cargo.toml.orig`). Everything else is the published crate unchanged. The first commit adding it holds the pristine copy. |

## Modified: `alacritty/` (crate `alacritty_embed`)

Copied from upstream `alacritty/` (`src/`, `res/`, `build.rs`, `Cargo.toml`) and turned into a
library that runs upstream Alacritty's display, OpenGL renderer, shaders, glyph cache
(crossfont), bindings, input, hints, search, Vi mode and event processing inside
embedder-owned AppKit `NSView`s instead of a winit window and event loop. Each embedder
session has its own processor, PTY, terminal, view, GL context and renderer; every view
callback, proxy wakeup and timer carries its session key (see `src/embed.rs`). On Linux the
same registry hosts GTK input widgets with Wayland subsurfaces instead (see "Linux platform").

Byte-identical to upstream (all upstream files in `res/`, and):
`src/config/{bell,color,cursor,debug,font,general,mouse,scrolling,selection,serde_utils,terminal,window}.rs`,
`src/daemon.rs`, `src/display/{bell,color,cursor,hint,meter}.rs`,
`src/macos/{mod,locale,proc}.rs`, `src/renderer/` except the files
changed by the graphics patch and native corner masking (see below), `src/string.rs`.


Modified upstream files (each carries a header describing the change):

| File | Change |
|---|---|
| `Cargo.toml` | Library crate `alacritty_embed`; no CLI/IPC/config-watch/YAML deps; objc2 0.5 features for the view; `smallvec` for the graphics patch. Linux-only: upstream's Wayland features (`glutin/egl`+`wayland`, `winit/wayland`, no dlopen/X11; not `copypasta/wayland`), GTK 0.18 (`gtk`, `gdkwayland-sys`), `wayland-client`/`wayland-backend` (`client_system`) and `xkbcommon-dl`. |
| `build.rs` | GL binding generation unchanged except for the graphics patch's `GL_ARB_clear_texture`; removed git version env and Windows resources. |
| `src/clipboard.rs` | macOS keeps the default provider. Linux uses the GTK host's clipboard and primary selection (`GtkClipboard`; text fetched on every owner change, since Alacritty reads synchronously) instead of upstream's smithay Wayland clipboard, whose second `wl_data_device` did not receive selections on Hyprland; no X11 clipboard. |
| `src/config/mod.rs` | Parses one embedder-supplied TOML document; no file discovery/imports/migration. embed: strict type validation (including nested fields) rejects atomic reloads instead of applying invalid defaults; tests cover malformed TOML, fonts, scrolling, cursors and bindings. |
| `src/config/ui_config.rs` | Added one effective-shell getter, shared by PTY construction and session-state reporting, preserving modern/deprecated config precedence. embed: invalid bindings/hint fields propagate errors during strict host validation; unchanged forgiving behavior otherwise. |
| `src/config/bindings.rs` | embed: removed `Action::ClearLogNotice` and its default Ctrl+L binding with the unreachable standalone message bar. The separate Ctrl+L `ReceiveChar` binding remains, preserving PTY forwarding. |
| `src/display/mod.rs` | No window size request/show/focus/shadow/resize increments; refresh rate from the view's screen (NSScreen/GdkMonitor); `request_frame` after every frame also for Wayland handles (the embedded Linux view has no winit frame callbacks); `mark_surface_dirty`; graphics hunks of the graphics patch (see "Graphics"). embed: removed unused upstream `gl_context` and `make_not_current` methods (embed context switching uses `make_current`), message-buffer arguments, message-bar line reservation and message-background/text rendering; only search reserves lines now. |
| `src/display/damage.rs` | embed: removed message-bar-only `FrameDamage::add_viewport_rect`, its `rects` storage/reset/iterator chain, `viewport_y_to_damage_y`, and the `add_viewport_damage` unit test after removing their sole production caller. Terminal-line damage and the renderer's used `damage_y_to_viewport_y` conversion remain. |
| `src/display/window.rs` | Rewritten: same API surface, implemented on the embedded `TerminalView` and its NSWindow; the window ID is the view's session key and redraw requests wake only that session. Linux: the GTK/Wayland view (own `wl_surface` as the EGL window handle), toplevel actions through GTK, refresh rate from the GdkMonitor; macOS-only methods stay macOS-only. embed: removed unused macOS `set_maximized`/`set_fullscreen` startup setters and the resulting unused `NSWindowStyleMask` import; live toggle actions remain, and initial window presentation is host-owned. |
| `src/event.rs` | winit `ApplicationHandler` replaced by an embed `Processor` (one per session) with the same routing; embedder-supplied PTY options; terminal exit keeps the view; becoming occluded ends mouse drags; no new instances/windows. embed: processor config updates, OSC 777 host-event interception and BEL forwarding without removing visual bell/urgency/bell-command behavior. `EventProxy` diverts notifications to a producer-bounded lane before the generic proxy queue; paused-consumer flood tests cover count/bytes, FIFO and unaffected terminal events. The view type is the platform's; `ActiveEventLoop` and `perform_action` are macOS-only. embed: removed unused `Processor::config`, `TOUCH_ZOOM_FACTOR`, all `TouchPurpose` variants/default and `TouchZoom` state/methods, the touch field/accessor in `ActionContext`, `EventType::Message` and its handling, message-buffer field and message/pop accessors, and touch-only imports. Neither native host produces touchscreen events, and no code produces standalone message-bar notices. Window-event construction no longer carries the host wrapper's unused window ID; terminal event IDs and session routing remain. |
| `src/input/mod.rs`, `src/input/keyboard.rs` | Host key/modifier/event types; `execute_action` (macOS); winit-based unit tests not carried over. embed (`mod.rs`): removed `MAX_TAP_DISTANCE`, `touch`/`on_touch_start`/`on_touch_motion`/`on_touch_end`, `ActionContext::touch_purpose`, message/pop trait methods, the `ClearLogNotice` action handler, message-bar click interception/cursor calculation and related imports. Mouse selection, wheel/trackpad scrolling and keyboard font zoom remain; neither host produces touchscreen events. |
| `src/scheduler.rs` | Host `EventLoopProxy`/`WindowId` imports. |
| `src/window_context.rs` | `new_embedded` with embedder PTY options, retained PTY thread/child state, reap-safe non-blocking shutdown plus bounded wait. Linux EGL uses the view's Wayland display; macOS Edit-menu helpers are macOS-only. embed: 128-entry drop-oldest host FIFO, normal exit-code accessor, live foreground-group query, and `update_config` re-ported from def47a5 (colors/bell, fonts with runtime-zoom preservation via pending update, cursor thickness/blinking, padding, terminal options/scrolling, title, hints, macOS Option-as-Alt). Host-owned window theme/opacity/blur/shadow/resize-increment and IPC policies are intentionally omitted. |

Removed upstream files: `src/main.rs`, `src/cli.rs`, `src/ipc.rs`, `src/logging.rs`,
`src/migrate/`, `src/panic.rs`, `src/config/monitor.rs`, `src/message_bar.rs` (embed: entire
standalone message model/types/buffer/formatting and its unit tests; no embed code ever
constructs or pushes these notices, and host settings errors are displayed by the app).

New files (not upstream Alacritty): `src/lib.rs`, `src/host.rs` (its `EventLoopProxy` wakes
carry the owning session key), `src/embed.rs` (multi-session registry: per-session pumps,
timers, viewport/hide/focus, stop, remove and bounded concurrent shutdown; every session enables
the PTY exit drain like upstream `--hold` (`drain_on_exit |= hold`); `setup_environment`
calls upstream `alacritty_terminal::tty::setup_env` like upstream Alacritty's `main`; the
registry is platform independent and reaches native views and main-loop callbacks through
`src/platform/{macos,linux}/embedder.rs`), `src/platform/mod.rs`,
`src/platform/macos/embedder.rs` (AppKit/libdispatch side of the registry, moved out of
`src/embed.rs` unchanged in behaviour) and the Linux files below.

embed: `src/host.rs` removes unused winit-shaped left/right shift, control and super
state accessors; left/right Alt accessors, `mod_state` and `ModifiersKeyState` import
are cfg-gated to macOS, the only consumer (Option-as-Alt). Its `Touch` re-export is
removed with touchscreen support. `WinitEvent::WindowEvent::window_id` is removed:
the processor/context already owns exactly one session and never reads this redundant
field. `src/lib.rs` no longer declares the removed message-bar module. `src/window_context.rs`
removes touch/message-buffer fields, initialization and action/display-update plumbing;
its existing upstream search viewport adjustment and redraw routing remain unchanged.

## Graphics: Sixel and iTerm2 inline images

Sixel support is ayosec's Alacritty graphics patch, https://github.com/ayosec/alacritty
(Apache-2.0, like Alacritty). The diff `v0.16.0..v0.16.0-graphics` (tag commit
`596631c11b1b625c3e0eb26161f90ede4ca84b32`), limited to `alacritty/` and `alacritty_terminal/`,
was applied to def47a5 (31 upstream commits before v0.16.0) with a three-way merge. Conflicts:
`alacritty_terminal/Cargo.toml` keeps def47a5's `unicode-width = "0.1"` (v0.16.0's bump to 0.2 is
not part of the patch) and `src/grid/row.rs` (context only). The later tags
`v0.16.1-graphics` (`40a6cee4c77b865d23154153c5b465069e170a67`) and `v0.17.0-graphics`
(`12d0fada4482eafde0a45fa4489e03b4d96b7190`) were compared: their graphics hunks in upstream files
are identical, and the only change to graphics files is `39d94d1` ("Remove custom abs_diff
function", a `sixel.rs` test cleanup), which is applied. Everything else in those tags is
upstream Alacritty releases.

`vte` is replaced by the `vte-graphics` 0.15.0 crate (https://github.com/ayosec/vte-graphics,
Apache-2.0 OR MIT): vte 0.15.0 plus the `dcs_hook`/`dcs_put`/`dcs_unhook`, `graphics_attribute`
and `osc_unhandled` handler hooks. Cargo renames the package, so it is still imported as `vte`.
It is vendored in `vendor/vte-graphics/` with APC hooks added for kitty graphics (see "Core crates").

iTerm2 inline images (OSC 1337 `File=...;inline=1:<base64>`) are a port of
`alacritty_terminal/src/graphics/osc1337.rs` from ayosec's experimental 2021 tag
`graphics-with-osc1337-v1` (`ca3633783e7e1160a61a42c3b27ee537c15493b2`, Apache-2.0) onto the
current graphics module: it is called from `Term::osc_unhandled`, decodes with `base64` 0.22 and
`image` 0.25 (MIT OR Apache-2.0; bmp, gif, jpeg, png and tiff decoders; no WebP, whose decoder
exceeds the allocation limit) under decoder limits, resizes to the requested `width`/`height`
(`N` cells, `Npx`, `N%`, `auto`) and `preserveAspectRatio` with its own box-filter shrink and
bilinear enlargement (no full-size RGBA or `f32` intermediates), and inserts the result like a
Sixel graphic. The multipart form (`MultipartFile`/`FilePart`/`FileEnd`), non-inline transfers
and other 1337 commands are not supported. Decoding runs synchronously on the PTY thread.

| File | Origin |
|---|---|
| `alacritty_terminal/src/graphics/mod.rs` | Graphics patch. Changed: declares `osc1337`; `insert_graphic` ignores graphics when the terminal has no cell size, where it would otherwise panic; the upload queue drops graphics already removed from the grid and is capped at 256 MiB (hidden sessions are never drawn, so they never take it). |
| `alacritty_terminal/src/graphics/sixel.rs` | Graphics patch, with `39d94d1`. |
| `alacritty_terminal/src/graphics/osc1337.rs` | Port of the `graphics-with-osc1337-v1` module (see above). |
| `alacritty_terminal/tests/sixel/` | Graphics patch fixtures (README, 3 Sixel images, RGBA references). |
| `alacritty_terminal/tests/iterm2/chafa.iterm` | New: `chafa -f iterm -s 4x2` output (uncompressed TIFF payload). |
| `alacritty_terminal/Cargo.toml` | `vte-graphics`, `smallvec`, and `image` for OSC 1337. |
| `alacritty_terminal/src/{lib.rs,grid/row.rs,term/cell.rs}` | Graphics patch. |
| `alacritty_terminal/src/grid/mod.rs` | Graphics patch; `truncate` doc comment updated for its new caller. |
| `alacritty_terminal/src/term/mod.rs` | Graphics patch, plus `osc_unhandled` for OSC 1337; clearing the scrollback (`CSI 3 J`) frees the purged rows (`Grid::truncate`) instead of caching them, so graphics on them are released. |
| `alacritty/res/graphics.{v,f}.glsl`, `alacritty/src/renderer/graphics/{mod,draw,shader}.rs` | Graphics patch (new files; source header added). |
| `alacritty/src/display/content.rs`, `alacritty/src/renderer/{mod.rs,rects.rs}`, `alacritty/src/renderer/text/{mod,gles2,glsl3}.rs` | Graphics patch; no longer byte-identical to upstream. |
| `alacritty/src/display/mod.rs`, `alacritty/build.rs`, `alacritty/Cargo.toml` | Graphics hunks only (see the table above). |

## Graphics: kitty graphics protocol

New code written from the kitty graphics protocol specification
(https://sw.kovidgoyal.net/kitty/graphics-protocol/); no kitty, Ghostty or Rio source is used.
Supported: direct (`t=d`) transmission of
PNG (`f=100`, decoded with the OSC 1337 helpers) and raw RGB/RGBA (`f=24`/`f=32`), chunking, the
`a=q` query, `a=t`/`a=T`/`a=p`, Unicode-placeholder virtual placements (`U=1`), direct placements
with `x,y,w,h` crop, `c`/`r` fitting and `C=1`, the `a=d` delete forms and replies per `q`. Not
supported: file/shared-memory transmission, `o=z`, animation, relative placements, `X`/`Y` pixel
offsets and z-index layering. One image store serves both screens; direct placements use Sixel
cell semantics (covered text is replaced). Memory bounds: 320 MiB / 4096 source images; 256 MiB /
4096 placement textures (identical virtual placements share one texture); textures of live
placements are exempt from the 256 MiB Sixel upload cap and the renderer's 1000-texture eviction.

| File | Origin |
|---|---|
| `alacritty_terminal/src/graphics/kitty.rs` | New: command parsing, chunk reassembly, decoding, image store and quotas, placements, deletes, replies, damage flag. |
| `alacritty_terminal/src/graphics/placeholder.rs` | New: placeholder cell decoding and inheritance. The 297-entry row/column diacritic table lists the code points of kitty's `gen/rowcolumn-diacritics.txt` (kovidgoyal/kitty `7d374531f9adcd0a6df018684e61d9b799ea10e2`) as data; no kitty code. |
| `alacritty_terminal/src/graphics/mod.rs` | Also: declares `kitty`/`placeholder`; `Graphics.kitty`; `attach_graphic_row` split out of `insert_graphic` for direct placements; separate uncapped `retained` upload queue for kitty placement textures (`UpdateQueues.retained`). |
| `alacritty_terminal/src/graphics/osc1337.rs` | Also: decoding/scaling helpers are `pub(crate)` for kitty; OSC 1337 behaviour unchanged. |
| `alacritty_terminal/src/term/mod.rs` | Also: APC hooks feed kitty, replies via `Event::PtyWrite`, `ESC c` resets kitty state, a command that changed placements fully damages the screen; `kitty_virtual_placement` accessor. |
| `alacritty_terminal/Cargo.toml` | `vte` points at `../vte-graphics`. |
| `alacritty/src/display/content.rs` | Also: decodes placeholder cells from raw colours before selection/search/inverse, hides the placeholder glyph and diacritics, emits a kitty tile. |
| `alacritty/src/display/mod.rs` | Also: a frame uploading kitty placement textures is fully damaged. |
| `alacritty/src/renderer/graphics/{draw,shader}.rs`, `alacritty/res/graphics.v.glsl` | Rewritten vertex path: quads and texture coordinates computed on the CPU, one quad per placeholder tile sampling a sub-rectangle; Sixel/OSC 1337 keep the same geometry. |
| `alacritty/src/renderer/graphics/mod.rs` | Also: textures of kitty placements are exempt from the 1000-texture eviction, which now sorts candidates once. |
| `alacritty/src/embed.rs` | `setup_environment` sets `PI_FORCE_IMAGE_PROTOCOL=kitty` and `PI_KITTY_PLACEHOLDERS=1` for PTY children unless already set (OMP opt-in). |

## Derived from winit 0.30.9 (Apache-2.0)

https://github.com/rust-windowing/winit, crate version 0.30.9 (the version upstream Alacritty
pins). Files in `alacritty/src/platform/macos/`:

| File | winit source | Change |
|---|---|---|
| `view.rs` | `src/platform_impl/macos/view.rs` | See file header: embed host events routed by the view's session key, sibling-view focus/occlusion/scale, Edit menu actions. embed: physical kVK + exact semantic-modifier conversion and the host-supplied shortcut router, shared with the AppKit local monitor. When the router declines a native-menu chord, preserve the base view's exclusions (Cmd+Q/W/M/T/1..9/H, Cmd+Option+H, Cmd+Shift+physical brackets) and pass it through to AppKit; Hide Others remains native-menu owned. embed: `mouseDownCanMoveWindow` returns NO, because the host's titled window overlays a transparent titlebar on the content and AppKit would otherwise let a (rounded, hence non-opaque) terminal view drag the window instead of selecting. |
| `event.rs` | `src/platform_impl/macos/event.rs` | Produces host `KeyEvent`/`Modifiers`; public scancode trait; local CF FFI. |
| `cursor.rs` | `src/platform_impl/macos/cursor.rs` | Custom RGBA cursors removed. |
| `ffi.rs` | `src/platform_impl/macos/ffi.rs` | Keyboard layout subset; adds libdispatch main-queue FFI. |

`alacritty/src/host.rs` reproduces the shape of winit 0.30.9's `KeyEvent`, `Modifiers` and
`ModifiersKeys` (`src/event.rs`, `src/keyboard.rs`), whose fields are private in winit.

## Linux platform (GTK 3 on Wayland)

Files in `alacritty/src/platform/linux/` (Linux-only; macOS builds compile none of them):

| File | Source | Content |
|---|---|---|
| `mod.rs` | New | GDK Wayland display/surface access (`gdkwayland-sys`), toplevel helpers. |
| `embedder.rs` | New | Linux side of the `src/embed.rs` registry: moves the `WebKitWebView` into a `GtkOverlay` (replacing Tauri's container as the window's child), guest `wayland-client` connection on GTK's `wl_display` with its own queue, CSS-to-GTK pixel scale (WebKitGTK font-DPI page zoom), GLib idle/timeout callbacks. |
| `view.rs` | New, following the event semantics of winit 0.30.13's Wayland backend (Apache-2.0) | Per-session input-only `GtkEventBox` + `GtkIMMulticontext` and own `wl_surface`, mapped as a desynchronized `wl_subsurface` (empty input region) of the toplevel while shown; GDK → host event translation (scroll gesture phases as winit's Wayland pointer), cursor names, IME. |
| `keyboard.rs` | `keysym_to_key`/`keysym_location` verbatim from winit 0.30.13 `src/platform_impl/linux/common/xkb/keymap.rs` (Apache-2.0, https://github.com/rust-windowing/winit); `control_transform` ported from `XkbToControl` in libxkbcommon `src/state.c` (https://github.com/xkbcommon/libxkbcommon, HPND AND MIT, originally X.Org Xlib) | GDK key events → host `KeyEvent` after winit's `process_key_event`; libxkbcommon Control transformation. |


## Edge-to-edge native corner masking (2026-10-04)

New files and changes to existing files are distinguished below; existing upstream
licenses remain unchanged.

### New application-owned files

| File | Change and rationale |
|---|---|
| `alacritty/src/renderer/corners.rs` | New Linux-only shader/renderer: two scissored static quads multiply completed premultiplied RGB and alpha by outer-quadrant circular coverage, leaving straight edges opaque; no per-frame upload/allocation. |
| `alacritty/src/platform/macos/corner_mask.rs` | New AppKit/QuartzCore helper: layer corner radius and left-only masked-corner bits in the layer's coordinate orientation; unequal nonzero radii use a CoreGraphics path/CAShapeLayer with correctly typed/encoded `CGPath` pointers. Disables implicit animation on geometry changes. |

### Modified existing application-owned files

| File | Change and rationale |
|---|---|
| `alacritty/src/embed.rs` | `ViewportRect` carries independent top/bottom-left CSS-pixel radii; rejects nonfinite/negative geometry, clamps each radius to half the smaller side, with a focused boundary regression. Radius-only reports trigger full repaint/damage so shrinking or removing an arc clears its prior transparency. |
| `alacritty/src/platform/linux/embedder.rs` | Scales radii with WebKit CSS page zoom and suppresses arcs at clipped edges; pure geometry regressions cover native-pixel quantization and the frontend's half-CSS-pixel rounding tolerance at fractional page zoom. Never rounds a genuinely clipped rectangle corner. |
| `alacritty/src/platform/linux/view.rs` | Stores logical left radii, exposes buffer-scaled pixel radii, explicitly keeps the Wayland opaque region unset so coverage reveals web chrome. |
| `alacritty/src/platform/macos/mod.rs` | Declares the native layer mask module. |
| `alacritty/src/platform/macos/embedder.rs` | Applies point-space radii at the terminal frame and suppresses arcs where the rectangle is clipped. |

### Modified upstream-derived files

| File | Change and rationale |
|---|---|
| `alacritty/src/renderer/platform.rs` | Requires at least eight alpha bits for Wayland EGL surfaces, including fallback configs, for antialiased transparent corners. |
| `alacritty/src/renderer/mod.rs` | Owns and dispatches the Linux corner renderer in the existing GL renderer lifecycle. |
| `alacritty/src/display/mod.rs` | Masks after all terminal content/graphics/overlays and before swap; includes both enabled corner squares on partial-damage swaps, using reusable damage storage so reused EGL buffers cannot expose stale corner pixels. |
| `alacritty/src/display/damage.rs` | Adds reusable output for shaped damage; existing debug damage behavior remains unchanged. |
| `alacritty/src/display/window.rs` | Makes the existing terminal-view accessor available on Linux for radius lookup. |
| `alacritty/src/platform/macos/view.rs` | Derived from winit (see above): tracks corner radii and makes rounded views nonopaque; dispatches the layer mask, rebuilding shape paths on frame/bounds notifications before waiting for frontend reports, including hidden autoresizes. |


## Agent harness embed boundary

All changes below are marked `embed:` in the affected source.

| File | Change |
|---|---|
| `alacritty/src/embed.rs` | Launch-scoped environment overrides (no global mutation); `TerminalState.exit_code`; `HostEvent::{State, Notify, Bell}` / `HostListener` replace the state-only callback; drains the bounded host FIFO outside borrows while retaining the pump reentry guard; replaceable configuration with validate/reload APIs and preflight borrow checks; foreground-job query/name (`/proc/<pgid>/comm` or `proc_name`); shared `AppKey`/`NamedKey` and macOS event conversion/filter API. The conversion, `unsafe fn app_key_from_macos_event(NonNull<c_void>)`, takes the raw `NSEvent *` (valid for the call, main thread) so the embedder's objc2 generation (0.6, as Tauri's) need not match this crate's objc2 0.5. |
| `alacritty/src/host.rs`, `alacritty/src/event.rs` | Producer-side OSC notification lane capped at 128 entries and 256 KiB of payload, dropping oldest notifications before enqueueing; generic terminal events remain unaffected. Sequence-ordered drain preserves FIFO across both lanes, and close rejects future notifications. Flood regression pauses consumption while sending 10,000 small / 1,000 maximum-size payloads. |
| `alacritty_terminal/src/event.rs` | New `HostNotify(String)` for OSC 777; Debug redacts its payload, including with Alacritty's print-events option enabled. |
| `alacritty_terminal/src/term/mod.rs` | OSC 777 `osc_unhandled` hook rejoins parameters after the numeric selector, bounds payload to 16 KiB, rejects invalid/truncated UTF-8, never logs agent payloads, forwards to `HostNotify`. Real ANSI-parser regression covers JSON semicolons; hook cap/debug-redaction tests added. Existing OSC 1337/kitty paths unchanged. |
| `alacritty_terminal/src/tty/unix.rs` | embed: Linux foreground shutdown enumerates `/proc/<pid>/stat` group/session/tty/starttime fields, opens matching member pidfds with `pidfd_open`, and revalidates starttime and membership after opening. It collects and validates the complete set before any `pidfd_send_signal`, then delivers to every verified member without aborting on foreground reclamation or session-leader exit. The same pidfds are retained through bounded SIGHUP→SIGKILL escalation; pidfd readiness detects exit without numeric-PID lookups or reaping. No numeric foreground `killpg` or `getsid(PGID)` leader requirement remains; unsupported/failed pidfds never fall back to numeric signalling. Unreaped-child checks and deadline-limited scans preserve bounded shutdown, with numeric `killpg` confined to the owned, unreaped shell group. macOS retains TIOCSIG. Owned FIFO regressions cover ignoring SIGHUP, a leaderless Linux foreground pipeline, and an ignored-SIGHUP same-group child surviving its parent until escalation after the shell reclaims the TTY and exits. Identity validation rejects changed starttime, session, tty and group. A real PTY regression covers normal exit 7 versus signal death (`ChildEvent::Exited(Some(7))` versus `None`) and already-reaped teardown. |

## Application icons

The assets in `src-tauri/icons/` were generated for this project: a blue terminal
chevron/underscore on a dark rounded square and a monochrome tray mark. They are
not copied from Alacritty or another application's artwork. The ICNS container
was generated with macOS icon tooling; ancillary image metadata has been removed.
