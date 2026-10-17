# Mukhomor Slint interface

Slint is the sole Mukhomor frontend. The normal Windows build opens it directly,
including after installation and autostart. There is no alternate renderer or
frontend selection flag. Linux uses a private controller process; Android uses
VpnService through JNI. See [platform integration](../docs/platforms.md).

## Structure

- `views/app.slint`: portable layout, original pixel surfaces, internal scroll
  viewports, dialogs and keyboard focus.
- `src/state.rs`: controller receipts, cancellation generations, conditional
  status, editable drafts and recovery state. No operating-system imports.
- `../native/src/slint_host.rs`: shared host with platform adapters for controllers, imports, preferences,
  file dialogs, tray, lifecycle and activation.
- `src/preview.rs`: isolated renderer with public synthetic data. It has no
  controller transport and never changes the network or imported profiles.
- `src/visuals.rs`: source palettes, icons, mushroom and background projection.
  Resources use `native/visual_build.rs`, `native/src/mushroom.rs`,
  `native/src/hero_motion.rs` and `native/src/appearance.rs`.

Background and surface pixels include the website's stationary four-tone grain
at 2% strength. It repeats every 144 × 96 field cells, follows the existing
13-pixel grain and UI scale, and is disabled with the visual effects. The noise
tile is cached once; animation interpolates the field underneath it without
regenerating or moving the grain.
- `../native/src/desktop.rs`: Windows instance activation and authenticated
  controller recovery, independent of drawing code.

## Design acceptance

The existing Mukhomor design is the reference, not a starting point for a new
design. Compare with the approved `assets/preview.png` and
`assets/appearance-preview.png` references at the same size, theme, locale and state.
The default window is 560 × 828, reserving 88 pixels for connection types above
the original hero/list composition. Preserve Tiny5 type hierarchy (32/24/16), continuous
pixel field, open mushroom interaction, compact 44-pixel server rows, pixel
icons, stepped corners and primary add button. Bottom navigation uses one
translucent pixel-stepped capsule aligned to the content's side margins, with
space between nearly square tabs, icons raised slightly above the captions and a monochrome
mushroom based on the logo for the connection page. Only the mushroom is enlarged
to match the other icons' visible height despite its source grid's empty rows.
Connection types use the same translucent capsule and color selection, with
pixel server, peer-link and mushroom icons. Own VPS retains the profile list;
P2P lists Tor and I2P, with a clear distinction between Internet access and
internal `.i2p` services. The subscription remains an in-development placeholder.
An existing connection can be stopped from any mode; switching the active P2P
network requires disconnecting. Mode selection keeps VPS profiles and the
selected server. Short screens use a compact strip and hero.
The main mushroom is 168 logical pixels high (previously 144) and sits 8 pixels
lower in the standard window. Its action caption, current connection/target and
two-line guidance form separate rows. Tabs never switch the active VPN or jump
back during polling. Pending requests retain their network/server target;
server switching, cancellation, recovery and uncertain receipts have explicit
feedback. Windows can reconnect on profile selection; portable platforms explain
that disconnection is required first. Notices scroll in the guidance area and
can be dismissed without shifting the server list.
Buttons and navigation use color feedback instead of focus/selection outlines.
Keyboard activation remains available; fields, choices, list rows and checkbox
indicators retain their focus cues. Toggle rows use only the checkbox's cue.
Do not introduce enclosing cards, smooth rounded rectangles or substitute
generic platform styling. Change geometry only to correct a demonstrated bug
or to accommodate smaller viewports and text scaling.

The corrected connection layout reserves a scrollbar lane inside the list.
Its width is identical with empty, short and overflowing models. Long names
elide before protocol metadata; the selected profile's check replaces its leading
arrow. Compact category labels use smaller text.
Lists, popups, editors and page scrolling share a stepped pixel scrollbar,
inset 16 logical pixels at both ends at 100% scale. Server and preset rows use
the full viewport height, with no extra top/bottom padding. Small corner masks
sample the surrounding animated window field to clip the list silhouette;
they are not used for overlay popups. Narrow thumbs use a one-step corner.
Dragging preserves the grab point. Modal close buttons use the original
8 × 8 pixel icon enlarged to 32 × 32, inside a 48 × 48 click target at 100%.
Checkboxes share a 28-pixel stepped square with a 2-pixel border and an integer
pixel check, including hover, pressed, keyboard focus and disabled states.
Preset checkboxes have a full 44-pixel hit target and Space/accessibility actions.
Simple confirmation dialogs use their own bounded height. Rules keep all six
categories visible; presets use compact rows; appearance has its own settings
view. New text fields use Slint TextInput with the existing palette and pixel
surface rather than Fluent controls. Further keyboard/IME qualification is
tracked below.

Visual review is required even when compilation and geometry checks pass.
`connection-reference.png` uses the same public server names and composition
as the original preview; the render suite also covers Paper and Glacier.

Slint is pinned to 1.18.1. Resources and Tiny5 are embedded. The desktop uses
Winit and the software renderer; there is no WebView. Existing mihomo and
Mycelium handling is unchanged. Slint's attribution is in Settings → About;
see `../Slint-LICENSE.txt` for its separate license.

Taskbar and tray icons sample only the visible mushroom bounds, using the same
pixel rasterizer as the executable icon. They fill the square system canvas;
the header retains its separate layout padding. Colors follow the theme.

## Implemented features

- Connection state, connect/cancel/disconnect/recovery requests, profile
  selection by ID, file/text import, rename and confirmed removal.
- Six rule sections, separate personal/preset ownership, domain source switches,
  section enablement, draft save and validation results.
- Preset creation, rename/description, enablement, template copies, restoration,
  import/export through the existing rules-only format and confirmed removal.
- Full/split routing and LAN preferences; autoconnect and Windows startup;
  theme/size/language/rounding, diagnostics and rule updates.
- Existing per-user/root instance guard, activation and installer retirement;
  closing to tray, explicit exit through controller cleanup. Failed cleanup
  keeps the UI open. Unsaved edits require an explicit save/discard decision
  before exit.

The state reducer rejects stale generations and foreign receipts, reconciles
lost mutation receipts through `last_operation`, keeps drafts across polling,
and does not replace newer edits when an earlier save finishes. Mutation RPCs
are not automatically retried. Ordinary polling and selecting a server do not
replace the list model or reset its scroll position.

## Build and inspect

From the repository root:

```powershell
cargo build --manifest-path native/Cargo.toml --locked
cargo test --manifest-path ui/Cargo.toml --lib --locked
cargo test --manifest-path native/Cargo.toml --locked slint_host::tests -- --test-threads=1
cargo run --manifest-path ui/Cargo.toml --features preview --bin mukhomor-preview -- --render target/slint-preview
cargo run --manifest-path ui/Cargo.toml --features preview --bin mukhomor-preview
```

The interactive preview contains fixtures only. The render command writes PNGs
for empty/short/512-item lists, both scroll ends, long names, narrow windows,
small/large text, dialogs, rules, presets and settings. It also checks scrollbar
clearance, thumb dragging, stable row width and navigation bounds. Generated
screenshots and build outputs are ignored by Git.
The host callback test uses an in-memory controller and checks preset ownership,
save/discard before exit, refresh dispatch and clearing imported text. It does
not open native windows, access the service or change network configuration.

To test the actual Windows adapter, use the built executable against a correctly
installed matching controller. The normal bootstrap and installation checks
still apply. `--qa-startup --base <fixture-base>
--root <fixture-root>` is reserved for isolated controller fixtures containing
`qa.marker`; it bypasses installation and uses fixture-local preferences.
Do not confuse the preview with a live VPN test.

`./Build-Release.ps1 -Offline` creates the Windows and source release packages.
Double-click `mukhomor.exe` to open Slint. The latest packaged runtime location
is recorded in `release-staging/<version>/windows/x64/latest-package.txt`. `tests/Test-SlintStartup.ps1` checks
the real window, isolated-root guard, second-instance activation and installer
retirement without connecting a VPN.

## Remaining work and qualification

1. Finish parity for connection-caption motion, animation speed controls,
   advanced DNS controls, rule search/selection/application picker, log-opening
   shortcuts and the remaining small interactions. Keep saved fields intact.
   Mushroom growth and background frames already use the original shared
   data/state machine; qualify timing and performance on real backends.
2. Complete automated UI interaction coverage: keyboard navigation and focus
   restoration, modal Escape/Tab behavior, scroll retention after structural
   list changes, IME/clipboard, every theme and text scale, dirty-exit decisions.
3. Run the Slint adapter against isolated real-controller lifecycle fixtures:
   imports, connect/cancel, DNS failures, service restart, lost receipts, startup,
   installer replacement and shutdown. State tests do not replace these checks.
4. Measure long rule editors, cold start, idle/tray CPU, memory and rendering on
   low-end hardware. Move more presentation/domain logic out of the Windows host.
5. Continue qualifying Linux service, routing, DNS, privileges and packaging, and Android VpnService/foreground lifecycle on additional real devices. Linux and Android builds, integration tests and their limitations are documented in `docs/platforms.md`.
6. Continue qualifying accessibility and actual devices. Making Slint the sole
   frontend does not turn outstanding platform or feature checks into passes.

Each migrated screen needs functional, visual and failure-path acceptance.
The presence of a control or a successful compile is not feature acceptance.
