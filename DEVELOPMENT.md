# Developer guide

For a quick setup guide, see [README.md](README.md).

## Installation details

Run `dist/nvidia-capture-in-ram-x64.exe`. It includes the official signed WinFsp
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

The default ceiling is 8192 decimal MB. Settings selects the ceiling and an unused
drive letter. **Apply and restart** discards RAM contents: save a wanted replay
first. Existing drive/ceiling configuration and lifetime counters are retained.
An old custom helper path is ignored; the matching bundled helper is always used.

Closing keeps recording active. Left-click the tray icon to reopen. The menu
provides Show, Stop and restore, and Quit. Stop and Quit restore NVIDIA's original
temporary path before unmounting and discarding RAM. If the tray cannot be
created, closing exits and the GUI explains the fallback.
Quit waits for cleanup to finish. If cleanup fails, the window shows the error
and offers **Retry shutdown** or **Exit anyway**. A failed restoration keeps its
recovery journal for the next launch; the original path can also be restored
through the NVIDIA overlay.

## Storage and accounting

[WinFsp-MemFs-Extended](https://github.com/Ceiridge/WinFsp-MemFs-Extended) supplies
RAM storage on top of the [WinFsp](https://github.com/winfsp/winfsp) Windows driver.
The GUI owns a Rust supervisor subprocess, which owns the native helper. Both
helpers run without a console window. The supervisor has a longer shutdown
deadline than the native helper; forced termination is reported as an error. See [vendor/UPSTREAM.md](vendor/UPSTREAM.md)
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
  ten seconds and at orderly shutdown. A failed checkpoint shows a warning and
  retries every ten seconds while recording continues. Abrupt termination can
  lose all bytes since the last successful checkpoint.
- Buffer allocation, helper working set and available system RAM are separate
  readings. MB/GB use decimal units. Memory is pageable, so this does not guarantee
  physical residency or zero disk writes. Saved clips and checkpoints intentionally
  use persistent storage.

The filesystem uses WinFsp's coarse operation guard to serialize callbacks and
capacity checks. File creation reports allocation failures without terminating
the volume. Renames prepare their namespace changes before committing and replace
destination alternate streams together with the file. Extending a truncated file
clears newly exposed bytes. NVIDIA compatibility and recording throughput need
native Windows testing.

## Discovery and recovery

The version-sensitive setting is `TempFilePath` in the current user's 64-bit
registry view at `Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS`.
UTF-16 string, expandable string and binary values are supported. Missing or
unsupported values produce a visible error. Original type and bytes are
preserved; existing disk directories are not moved or removed.

A journal is saved before redirection. Stop/quit restores before unmounting.
The GUI sends its exact recovery snapshot to the Rust supervisor before
readiness. The supervisor restores it on GUI control-pipe EOF, including GUI
crashes. Its own exit closes the native
helper's pipe, so MemFS unmounts. A subsequent launch recovers a pending journal
following a supervisor crash, before reading configuration or lifetime counters.
Restoration replaces only this app's exact value, preserving later user or NVIDIA
edits. Helper exit failures and invalid final telemetry are reported while the
last valid write counter is retained for accounting.

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
# Portable native regression harness (requires Python 3 and a C++20 compiler):
python3 tests/native/run.py
```

Portable lifecycle, storage, telemetry and registry representation tests run on
Linux without a driver or display. Windows adapters and GUI are target restricted.
Application code forbids unsafe Rust; native C++ uses WinFsp's API.
The native harness exercises production methods with platform stubs, including
allocation-failure injection and rename rollback; it does not replace driver
integration testing.

With Instant Replay off on a Windows NVIDIA machine, run
`./scripts/smoke-windows.ps1` after installation. It checks mount, overwrite
accounting, deletion, truncate/extend clearing, alternate-stream replacement,
capacity failure, owner EOF restoration and unmount.
Native Windows installation, NVIDIA recording, replay saving, tray controls,
crash recovery and capacity behavior still require testing on a Windows NVIDIA
machine.

## Verified builds and optional releases

The **Check** workflow builds the Windows package after the Rust and native
checks pass. Its `replay-in-ram-windows-x64` artifact contains the installer,
a ZIP of the complete distribution (including dependency sources and licenses),
and checksums. Successful push builds automatically receive GitHub build
attestations for all three files. Pull-request builds are downloadable but are
not attested or eligible for release promotion.

No release is created automatically. To promote a build you have tested:

1. Open its successful **Check** run on GitHub and copy the numeric run ID from
   the URL (`.../actions/runs/123456789`).
2. Open **Actions → Draft release from build → Run workflow**. Supply that run ID
   and a new version tag such as `v0.1.0`.
3. The workflow downloads the existing build, verifies each asset's attestation
   against this repository, the Check workflow and its source commit, then tags
   that commit and creates a draft release. It does not rebuild the installer.
4. Review the draft under **Releases**, edit the notes or mark a prerelease as
   appropriate, and publish when ready.

The promotion workflow must be present on the repository's default branch.
Choose a run made after attestation was enabled, while its artifact is still
available (requested retention: 90 days, subject to repository policy).
Existing tags are never overwritten. If draft creation fails after tagging,
the tag remains; finish that release manually rather than moving the tag.

Release notes include a verification command tied to the exact build commit.
For a repository-level check, replace `OWNER/REPO` below with the GitHub repository:

```sh
gh attestation verify nvidia-capture-in-ram-x64.exe --repo OWNER/REPO --signer-workflow OWNER/REPO/.github/workflows/check.yml
```

Attestations establish GitHub Actions build provenance; they do not establish
independently reproducible builds or provide Windows publisher code signing.
GitHub supports attestations for public repositories on current plans; private
repositories require Enterprise Cloud. No personal signing key is required.
