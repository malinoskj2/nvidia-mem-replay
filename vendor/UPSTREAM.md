# Native filesystem provenance

`memefs/` contains the WinFsp-MemFs-Extended implementation from
https://github.com/Ceiridge/WinFsp-MemFs-Extended at commit
`5ff06409150889171da6e2260564bdb2e37a8fa2` (2026-10-05 retrieval).
The original CLI and project files are retained. The GPL-3.0 license is included.

Local changes for Replay in RAM:

- `replay-main.cpp` replaces the service CLI in the build. It mounts a case
  insensitive NTFS-named volume with an explicit byte ceiling, publishes bounded
  JSON telemetry every 250 ms, and drains the dispatcher/unmounts on stdin
  command or EOF. WinFsp is loaded from its registered shared installation.
- `memfs.h` and `io.cpp` add an atomic cumulative successful-write counter.
  Overwrites and deleted files remain counted; failed callbacks do not add bytes.
  Buffer telemetry uses allocated 64 KiB sectors, including deleted-open files,
  rather than scanning directory entries or measuring file-size growth.
- `create.cpp` selects WinFsp's coarse operation guard to serialize callbacks.
  This protects namespace access and keeps concurrent capacity checks from racing.
  It also reports external dispatcher stops so the supervisor can restore NVIDIA.
  Throughput with NVIDIA still needs native measurement.
- `nodes-compat.cpp` and `filecreate.cpp` account for allocation rounding and
  sector-pointer overhead before allocating and bound file sizes before alignment.
  `sectors.cpp` avoids dereferencing the singleton for an empty/moved root node
  during filesystem construction.
- `MemFs` moves are explicitly deleted: a mounted instance owns a singleton address
  and atomics. The project uses the static C++ runtime, accepts a `WinFspSdk`
  build property, and builds the owned adapter rather than the service CLI.

The owned Rust `filesystem` subprocess retains NVIDIA snapshot/restoration and
forwards telemetry from the bundled `memefs-x64.exe`.

WinFsp 2.1.25156 is supplied by its official signed MSI. The build downloads that
MSI and the matching v2.1 source archive with pinned SHA-256 hashes. The shared
runtime stays installed after application uninstall. WinFsp's GPL license with
its redistribution exception is included in `licenses/`; the app and MemFS
Extended remain GPL-3.0-only. Matching application, native helper and Rust
dependency sources accompany the runtime package.
