# Mukhomor Windows host

Slint is the only frontend and opens by default. Build the distributable with
`Build-Release.ps1`; no UI feature flag or launch argument is required.
See [the UI architecture and qualification notes](../ui/README.md).

`src/slint_host.rs` binds the portable presentation layer to the authenticated
Windows controller, file dialogs, preferences, autostart and tray. Controller
IO runs outside the event loop. `src/desktop.rs` owns per-user/per-root instance
activation and checked controller recovery. The activation class and installer
retirement messages remain compatible with installed versions.

`src/design.rs` retains the saved appearance schema and bounds. The palette,
artwork and animation data are shared with Slint; no GDI control renderer is
compiled. The header uses padded artwork, while taskbar, tray and executable
icons fit the visible mushroom to the full canvas.

## Controller and persistence

The controller handles `Rename` and `Autoconnect` as serialized Rust metadata transactions after successful initialization. Each transaction pins and checks the state directories and profile index, bounds and validates the library, and atomically replaces it before updating the public snapshot. Activation, import, settings validation and network recovery continue through the supervised bridge. Uncertain initialization uses the existing bridge path.

`Status` accepts an optional `if_revision` token. Matching snapshots return only `ok`, `not_modified` and `revision`; callers without a token retain the full response. Every mutable cache access invalidates the token, and controller lifetimes have distinct tokens. The UI clears its token on mutation admission, replies and transport failures, preserves drafts on compact replies, and refetches on mismatches.

Settings application reuses the diagnostics from the rule projection used to build the core configuration. The diagnostics are published only after core validation and the settings transaction succeed; the stored settings retain the user's original entries. `tests/Test-NativeApp.ps1 -Executable <release-exe> -LargeRulesOnly` runs the 15,000-entry IPC regression separately and reports its elapsed time, including a check that empty diagnostics remain an empty JSON array.

## Verification

```powershell
cargo test --manifest-path native/Cargo.toml --locked -- --test-threads=1
cargo test --manifest-path ui/Cargo.toml --lib --locked
cargo run --manifest-path ui/Cargo.toml --features preview --locked --bin mukhomor-preview -- --render target/slint-preview
.\tests\Test-SlintStartup.ps1
```

The preview uses synthetic data. The startup test creates an isolated QA root,
checks rejection without its marker, starts the actual Slint window, verifies
second-instance activation and retires its own process. It never connects VPN.
Use `-Executable <packaged-exe> -Base <package-folder>` to inspect a release.

`tests/Measure-NativeControl.ps1 -Executable <release-exe> -VerifyNative -Output <json>`
measures authenticated IPC operations and controller readiness. It does not
measure Slint rendering or GUI startup. Previous GDI performance observations
must not be presented as measurements of this frontend.
