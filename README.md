# Replay in RAM

A Windows x64 app that finds NVIDIA Instant Replay's configured temporary-files
location, mounts a dynamically allocated RAM filesystem with **WinFsp and MemFS
Extended**, and redirects recording to `R:\NVIDIA-Replay`. The GUI shows write
activity, lifetime GB and allocated buffer MB. Closing hides it to the tray.

## Install and use

Run `dist/replay-in-ram-setup-x64.exe`. It includes the official signed WinFsp
2.1.25156 installer and the bundled `memefs-x64.exe`; no separate downloads are
needed. Driver installation requires administrator access and can require a
reboot. Launch **Replay in RAM** from the Start menu as the same ordinary Windows
user who runs the NVIDIA overlay.

A registered compatible WinFsp 2.1+ runtime is kept. An incompatible installation
may need removal and a reboot before the bundled MSI succeeds; installation
errors are reported. Uninstalling this application leaves shared WinFsp installed.
Remove WinFsp separately through Windows Apps when no other application needs it.

Set **Temporary files** once in **Alt+Z → Settings → Files and disk space** if
not configured. The app reads that setting each launch. Keep **Gallery** on an
SSD or another persistent drive. Toggle Instant Replay off/on if NVIDIA needs
to reload its cached settings.

The default ceiling is 4096 decimal MB. Settings selects the ceiling and an unused
drive letter. **Apply and restart** discards RAM contents: save a wanted replay
first. Existing drive/ceiling configuration and lifetime counters are retained.
An old custom helper path is ignored; the matching bundled helper is always used.

Closing keeps recording active. Left-click the tray icon to reopen. The menu
provides Show, Stop and restore, and Quit. Stop and Quit restore NVIDIA's original
temporary path before unmounting and discarding RAM. If the tray cannot be
created, closing exits and the GUI explains the fallback.

## Storage and accounting

[WinFsp-MemFs-Extended](https://github.com/Ceiridge/WinFsp-MemFs-Extended) supplies
RAM storage on top of the [WinFsp](https://github.com/winfsp/winfsp) Windows driver.
The GUI owns a Rust supervisor subprocess, which owns the native helper. Both
helpers run without a console window. See [vendor/UPSTREAM.md](vendor/UPSTREAM.md)
for the pinned source and local adapter changes.

- File data is allocated on demand in 64 KiB sectors. The byte ceiling is passed
  to MemFS Extended, whose capacity estimate includes sectors, pointers and
  estimated node metadata. The displayed buffer is allocated file-data sectors,
  including preallocation and sector rounding. It is not total process memory.
- Lifetime counts successful filesystem callback bytes, including overwrites
  and files subsequently deleted. Cached writes may be combined before reaching
  the callback. Failed writes do not count. All writers on this dedicated volume
  contribute, not just NVIDIA. This is not a measurement of avoided SSD writes.
- Activity stays lit for two seconds after a write increase. Cumulative telemetry
  preserves bytes across skipped GUI samples. Lifetime checkpoints occur every
  ten seconds and at orderly shutdown; abrupt termination can lose recent bytes.
- Buffer allocation, helper working set and available system RAM are separate
  readings. MB/GB use decimal units. Memory is pageable, so this does not guarantee
  physical residency or zero disk writes. Saved clips and checkpoints intentionally
  use persistent storage.

The filesystem uses WinFsp's coarse operation guard to serialize callbacks and
capacity checks. Its NVIDIA compatibility and recording throughput need native
Windows testing. MemFS Extended's filesystem semantics replace the former custom
Rust filesystem; the GUI and recovery behavior remain the same.

## Discovery and recovery

The version-sensitive setting is `TempFilePath` in the current user's 64-bit
registry view at `Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS`.
UTF-16 string, expandable string and binary values are supported. Missing or
unsupported values produce a visible error. Original type and bytes are
preserved; existing disk directories are not moved or removed.

A journal is saved before redirection. Stop/quit restores before unmounting.
The Rust supervisor snapshots the original value before readiness and restores
it on GUI control-pipe EOF, including GUI crashes. Its own exit closes the native
helper's pipe, so MemFS unmounts. A subsequent launch recovers a pending journal
following a supervisor crash. Restoration replaces only this app's exact value,
preserving later user or NVIDIA edits.

State is in `%LOCALAPPDATA%\NvidiaMemReplay`. A single-instance lock protects
`config.json`, `lifetime.json` and `redirect.json`. Replay data disappears when
the RAM filesystem stops; persistent clips retain their Gallery location.

## Build and verify

Windows builds require Rust 1.95+, Visual Studio 2022 C++ build tools and NSIS 3.
The script downloads and hash-verifies the pinned WinFsp MSI/source archive,
extracts SDK headers/import libraries without installing a build-host driver,
builds MemFS Extended with MSBuild and the static C++ runtime, and builds the
Rust app. The installed shared WinFsp runtime is loaded by the native helper.

```powershell
./scripts/build-windows.ps1
# Build the runtime/source bundle without the NSIS installer:
./scripts/build-windows.ps1 -SkipInstaller
```

The bundle includes licenses, matching WinFsp source and complete app/helper/Rust
dependency sources. Keep `memefs-x64.exe` beside `nvidia-mem-replay.exe`.

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Portable lifecycle, storage, telemetry and registry representation tests run on
Linux without a driver or display. Windows adapters and GUI are target restricted.
Application code forbids unsafe Rust; native C++ uses WinFsp's API.

With Instant Replay off on a Windows NVIDIA machine, run
`./scripts/smoke-windows.ps1` after installation. It checks mount, overwrite
accounting, deletion, capacity failure, owner EOF restoration and unmount.
See [docs/verification.md](docs/verification.md) for verification limits and
remaining native checks.
