# Phone Studio

A native Rust + Slint desktop workspace for owner-authorized Android servicing.
It runs real ADB/Fastboot commands through Google's separately installed
Platform-Tools. The simulated workspace is explicitly marked **DEMO** and never
starts either tool.

## Start

Install stable Rust and the native build prerequisites for
[Slint's winit backend](https://docs.slint.dev/latest/docs/slint/guide/platforms/desktop/).
On Windows, use the Rust MSVC toolchain and Visual Studio C++ Build Tools.
From /home/senu/PROJECTS/phone-studio:

```sh
cargo run --locked --manifest-path Cargo.toml -- --demo
```

Connect real phones by omitting `--demo`:

```sh
cargo run --locked --manifest-path Cargo.toml
```

Convenience launchers are also included: `bash run.sh --demo`
on Linux/macOS and `./run.ps1 --demo` in PowerShell. They use
a local `dist` executable when present, otherwise build/run through Cargo. Run
`./build-windows.ps1` on Windows to produce
`dist/phone-studio.exe`.

The app's **Setup** tab accepts the folder containing official `adb` and
`fastboot` executables. Install them from
[Google's Platform-Tools page](https://developer.android.com/tools/releases/platform-tools).
When no folder is selected, discovery checks Android SDK environment variables,
PATH and standard SDK locations. An explicitly selected folder never falls back
to a different tool on PATH. Windows needs the phone manufacturer's appropriate
USB driver; Linux may need USB/udev permissions.

To check executable versions without scanning or contacting phones:

```sh
cargo run --locked --manifest-path Cargo.toml -- --check-tools
```

## Working features

- Scan ADB and Fastboot devices, including unauthorized/offline/permission-error
  connections. Every targeted command uses an explicit serial, including when
  multiple phones are attached.
- Read selected Android properties or standard Fastboot product, lock, secure
  boot, bootloader version and unlock-permission queries.
- Collect the last 200 system-log entries through authorized ADB. View them in
  the separate Logs tab and save the collected text there. Collecting logs keeps
  the device report intact. Reports, logs, activity and setup text can be scrolled
  and selected for copying.
- Export selected device facts and query notes as JSON. Reports and system logs
  are stored only when explicitly exported; history stays in memory for the
  current session.
- Restart Android or Fastboot devices, enter the bootloader, or enter recovery
  through ADB. Reboots require confirmation by typing the selected serial.
- Request the standard official Fastboot bootloader unlock. This requires a
  report showing a product, `unlocked: no` and `get_unlock_ability: 1`.
  The operator must type the selected serial and `ERASE`; the app rechecks live
  eligibility and product identity before sending `fastboot -s SERIAL flashing
  unlock`. The phone requires its own confirmation. **Unlocking erases user data.**
  Command completion is not treated as proof of the final lock state: scan and
  read again to verify it.
- Cancel running tools, bound captured output and enforce command timeouts.
  The visible Close buttons request cancellation during a task and close
  automatically after the worker finishes. A command already accepted by a
  phone can still complete after its host process is stopped.

The official unlock workflow follows
[AOSP's locking/unlocking requirements](https://source.android.com/docs/core/architecture/bootloader/locking_unlocking).
Enable OEM unlocking in Android first if supported. Manufacturer token flows and
devices that do not answer the required standard queries remain unavailable.
An Android property alone never enables unlocking.

## Current scope

This is an initial functional release, with **no physical-device certification**.
Native Linux compilation, headless native rendering and simulated protocol tests
were verified during development. Windows source support is present but a Windows
build and physical phone tests have not been performed in this environment.

Samsung-specific flashing, iPhone services, firmware downloads/flashing,
manufacturer token integrations, screen PIN recovery, FRP/Activation Lock
removal and carrier unlocking are not implemented. The UI identifies Samsung
and iPhone support as planned. There is no universal-unlock claim.

The app uses the Rust/Slint native desktop stack rather than the
Tauri option in the original research. Before distributing a commercial build,
choose the appropriate [Slint license](https://slint.dev/license) and comply with
the separately installed tools' distribution terms. The complete source
dependency set is pinned in `Cargo.lock`.

## Settings and diagnostics

Only the tools-folder setting and session lock files are written automatically.
The default data directory is `%LOCALAPPDATA%\PhoneStudio` on Windows,
`$XDG_DATA_HOME/phone-studio` on Linux when set, or
`~/.local/share/phone-studio` otherwise. `PHONE_STUDIO_DATA` overrides it.
Corrupt settings are reported and preserved. Normal and demo sessions use
different locks. No account, server, telemetry or network service is configured.
Exported system logs can contain private app information; review before sharing.

## Build and test

```sh
cargo test --locked --manifest-path Cargo.toml
cargo fmt --manifest-path Cargo.toml --check
cargo clippy --locked --manifest-path Cargo.toml --all-targets -- -D warnings
cargo build --release --locked --manifest-path Cargo.toml
```

The release executable is in `target/release/` unless
`CARGO_TARGET_DIR` or `--target-dir` overrides it. On Windows its name is
`phone-studio.exe`. Platform-Tools and drivers are installed separately.

Tests cover explicit serial targeting, unauthorized phones, device replacement,
partial discovery failure, property filtering, failed unlock queries, changed
unlock permission/product identity, erasure confirmation, process cancellation
and timeouts, settings preservation, and actual native UI rendering. UI preview
PNGs are written to the operating system's temporary folder. No test contacts a
real phone or executes a real unlock.

The window opens at 1120×680 and can shrink to 960×520 for smaller or scaled
desktops. The workspace has a visible scrollbar so all controls remain reachable.
An unauthorized device explains why information is unavailable instead of
showing an empty report.

Development validation: 24 tests passed, formatting passed, the Linux development
binary built, and version queries succeeded with the installed Platform-Tools
37.0.0. Rendered previews were inspected down to 960×520. Regression tests dispatch
actual mouse-wheel events through the workspace, report and logs and verify that
unchanged UI refreshes preserve scroll position. The current
native Wayland desktop smoke test also passed: it dispatched mouse-wheel events,
verified scroll controls across regular UI refreshes, then clicked the visible
Close button and confirmed normal process exit. Clippy could not run because
the installed Rust toolchain lacks its component. The local `dist/phone-studio`
artifact, when present, is a Linux development build, not a Windows release.

Build 0.1.2 adds a Close button in the header, a Close app button at the bottom,
and persistent Scroll up / Scroll down controls for the active panel. Close an
older running window with Alt+F4 and relaunch to use the rebuilt executable.

For an automated native desktop smoke check using simulated data only:

```sh
./run.sh --smoke-ui
```
