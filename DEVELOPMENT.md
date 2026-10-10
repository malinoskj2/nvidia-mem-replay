# Developer guide

For a quick setup guide, see [README.md](README.md).

## Installation details

Run `dist/nvidia-mem-replay-setup-x64.exe`. It includes the official signed WinFsp
2.1.25156 installer and the bundled `memefs-x64.exe`; no separate downloads are
needed. Driver installation requires administrator access and can require a
reboot. Launch **nvidia-mem-replay** from the Start menu as the same ordinary Windows
user who runs the NVIDIA overlay.

A registered compatible WinFsp 2.1+ runtime is kept. An incompatible installation
may need removal and a reboot before the bundled MSI succeeds; installation
errors are reported. Uninstalling this application leaves shared WinFsp installed.
Remove WinFsp separately through Windows Apps when no other application needs it.

Set **Temporary files** once in **Alt+Z → Settings → Files and disk space** if
not configured. The app reads that setting each launch. Keep **Gallery** on an
SSD or another persistent drive. Toggle Instant Replay off/on if NVIDIA needs
to reload its cached settings.

The default ceiling is 8192 decimal MB; Settings selects it. The volume is
mounted at the directory `%LOCALAPPDATA%\NvidiaMemReplay\ram` as a WinFsp
directory mount point, so it has no drive letter and is absent from Explorer,
This PC, file dialogs and Disk Management. **Apply and restart** discards RAM
contents: save a wanted replay first. The ceiling and lifetime counters are
retained across versions; an old custom helper path or drive letter setting is
ignored (the supervisor's `--mount` still accepts a letter for the smoke script).
WinFsp removes the mount-point directory on unmount; one left behind by a killed
helper is a dangling reparse point, which the next start removes before mounting.

Closing keeps recording active. Left-click the tray icon to reopen. The menu
provides Show, Stop and restore, and Quit. Stop and Quit restore NVIDIA's original
temporary path before unmounting and discarding RAM. If the tray cannot be
created, closing exits and the GUI explains the fallback.
Tray menu and click events are delivered on the window's own thread, so the
handlers act on the window directly (`src/sys/tray.rs`).
Quit waits for cleanup to finish. If cleanup fails, the window shows the error
and offers **Retry shutdown** or **Exit anyway**. A failed restoration keeps its
recovery journal for the next launch; the original path can also be restored
through the NVIDIA overlay.

## Storage and accounting

[WinFsp-MemFs-Extended](https://github.com/Ceiridge/WinFsp-MemFs-Extended) supplies
RAM storage on top of the [WinFsp](https://github.com/winfsp/winfsp) Windows driver.
The GUI owns a Rust supervisor subprocess, which owns the native helper. Both
helpers run without a console window. The supervisor has a longer shutdown
deadline than the native helper; forced termination is reported as an error.
Unexpected or unsuccessful helper exits include their status and available
stderr diagnostics. Each helper retains at most 8 KiB while draining additional output, and
reports when the captured text was truncated. See [vendor/UPSTREAM.md](vendor/UPSTREAM.md)
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

The setting is ShadowPlay's `TempFilePath` property. The ShadowPlay engine
(hosted by `nvcontainer.exe`) reads it from the current user's 64-bit registry
view at `Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS` only when it
starts; later registry edits are ignored until the engine restarts, and toggling
Instant Replay does not re-read them. The overlay changes the value at run time
through NVIDIA's API library `nvspapi64.dll`, which forwards it over NVIDIA's
message bus to the running engine. The engine applies it immediately and
persists it, including the derived `HLTempPath`, to the registry.
`src/sys/shadowplay.rs` uses that same library: `CreateShadowPlayApiInterface`
with the first-generation interface version, then the `GetProperty`/`SetProperty`
table entries with a COM `VARIANT` (`VT_BSTR`). Each client id registers on
NVIDIA's message bus under its own module name and a duplicate registration is
dropped, so the GUI connects as the `TestingTool` client and the supervisor
process as the `Installer` client. The layout follows the open-source Experienceless client and was
verified against NVIDIA App 11.0.9; the engine logs every version or client-id
mismatch to `%ProgramData%\NVIDIA Corporation\ShadowPlay\CaptureCore.log`.
This is the only module with `unsafe` Rust of its own; the two other `unsafe`
blocks in the crate are single winsafe calls (a virtual-key constant in
`src/sys/hotkey.rs`, `EM_SCROLLCARET` for the Logs view in `src/gui/logs.rs`).

Discovery reads the live value from the engine, so the NVIDIA App must be
running. The registry is read only to preserve the original value type and
bytes for the journal.

Instant Replay opens its temporary files when capture starts, so a location
change takes effect only after an off/on cycle. `src/service/replay.rs` wraps
the redirection and the restoration in that cycle when Instant Replay is
capturing: the state comes from `GetCaptureSessionParam` (parameter 12 on the
engine-wide session handle), and the toggle is the user's own Instant Replay
on/off hotkey, read from `IRToggleHKeyCount`/`IRToggleHKey<n>` under
`NVSPCAPS` and pressed with `SendInput` (`src/sys/hotkey.rs`). NVIDIA's hotkey
helper forwards it to the overlay, which stops or starts Instant Replay on its
own capture session; the app never creates a session of its own. When Instant
Replay is idle nothing is pressed, and when the hotkey is unassigned or the
state cannot be read, the location change still happens and the window shows a
notice asking the user to toggle it.

A journal is saved before redirection. Stop/quit restores through the engine
before unmounting; when the engine cannot be reached, the registry value it
reads at its next start is restored instead.

Two situations are handled in the worker loop. When a start fails because the
engine is unreachable (the NVIDIA App has not started yet, typically at
sign-in), the worker shows "Waiting for the NVIDIA App" and retries every ten
seconds instead of reporting an error. Only the first engine call of a start,
reading the live location in `nvidia::plan`, is judged that way
(`NvidiaError::Unreachable`, from a failed library load, interface creation or
call); a value the engine refuses later, a missing library and internal errors
are reported as errors. While recording, it reads the live
location every five seconds: the overlay re-pushes its own stored copy of the
location (`GallerySettings.json`) whenever it starts, which silently undoes the
redirection, so a changed value is applied again with the usual Instant Replay
cycle and a notice is shown.

`--tray` starts the window hidden; the Start-with-Windows setting writes that
command line to the user's `Run` key (`src/sys/startup.rs`).

The window (`src/gui.rs` with one module per page under `src/gui/`, and the
shared layout helpers in `src/gui/layout.rs`) is built from the Windows common controls through
[winsafe](https://github.com/rodrigocfd/winsafe)'s `gui` module: a tab control
with two child pages, static labels, a combo box, an edit with an up-down
buddy, a check box and push buttons, all in the system message font. `build.rs`
embeds a manifest that opts into Common Controls 6 (visual styles) and DPI
awareness; without it the controls draw in the Windows 95 style. Group-box
frames are painted by the pages themselves with `DrawThemeBackground` and a
label for the title, because a child group-box control never erases its
interior and the pages clip their children, which left stale pixels inside the
boxes. Coloured status lines are tinted in `WM_CTLCOLORSTATIC`. The Logs tab is a
read-only multi-line edit fed every status tick from `src/log.rs`, a process-wide
journal (last 500 entries in memory, all of the session in
`%LOCALAPPDATA%\NvidiaMemReplay\nvidia-mem-replay.log`) that the worker, the
recovery path and the window write to at the points a user would want to see:
redirections, Instant Replay cycles, watchdog re-applies, stops, shutdown and
every error that reaches the status line.

The icon is rendered by `src/sys/icon.rs` for the tray, and the same renderer
writes `assets/nvidia-mem-replay.ico`, which `build.rs` embeds as icon
resource 1 (Start menu, Explorer, installer shortcuts, and the window class).
After changing the renderer, regenerate the file with
`cargo run -- icon assets/nvidia-mem-replay.ico`; a unit test fails while the
two differ. The title bar is set to the flat dialog colour with
`DwmSetWindowAttribute` (`src/sys/window.rs`), since Windows 11 would
otherwise tint it with the wallpaper.
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
Unsafe Rust is confined to `src/sys/shadowplay.rs`, the binding to NVIDIA's
ShadowPlay API library, plus one virtual-key newtype conversion in
`src/sys/hotkey.rs`; native C++ uses WinFsp's API.
The native harness exercises production methods with platform stubs, including
allocation-failure injection and rename rollback; it does not replace driver
integration testing.

With Instant Replay off on a Windows NVIDIA machine, run
`./scripts/smoke-windows.ps1` after installation (`-Directory <path>` exercises a
directory mount point instead of the drive letter). It checks mount, overwrite
accounting, deletion, truncate/extend clearing, alternate-stream replacement,
capacity failure, owner EOF handling and unmount. The supervisor's restoration
leaves a location that is not its own untouched, so the script verifies that
the live NVIDIA setting is unchanged rather than redirecting it.
Installation, filesystem behavior, tray controls and recovery can also be tested
in a Windows VM with a synthetic `TempFilePath` setting; the window uses the
standard Windows controls and needs no GPU. NVIDIA recording, overlay setting
reloads and actual replay saving still require a Windows NVIDIA machine.

## Verified builds and optional releases

The **Check** workflow builds the Windows package after the Rust and native
checks pass. Its `nvidia-mem-replay-windows-x64` artifact contains the installer,
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
gh attestation verify nvidia-mem-replay-setup-x64.exe --repo OWNER/REPO --signer-workflow OWNER/REPO/.github/workflows/check.yml
```

Attestations establish GitHub Actions build provenance; they do not establish
independently reproducible builds or provide Windows publisher code signing.
GitHub supports attestations for public repositories on current plans; private
repositories require Enterprise Cloud. No personal signing key is required.
