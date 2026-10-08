# nvidia-mem-replay

![Windows x64](https://img.shields.io/badge/Windows-x64-0078D4)
![NVIDIA Instant Replay](https://img.shields.io/badge/NVIDIA-Instant_Replay-76B900)
[![GPL-3.0 license](https://img.shields.io/badge/license-GPL--3.0-555555)](LICENSE)

**Use RAM for NVIDIA Instant Replay’s temporary recordings.**
Your saved clips still go to your usual Gallery folder.

You need **64-bit Windows**, an **NVIDIA GPU**, and **Instant Replay** working in the NVIDIA overlay.

## Get started

1. Run **`nvidia-mem-replay-setup-x64.exe`** from the Windows download. WinFsp is included; Windows may ask for admin access or a reboot.
2. Open the NVIDIA overlay with **Alt+Z**. In **Settings → Files and disk space**, set **Temporary files** to a folder if you haven’t already.
3. Keep **Gallery** on your SSD or hard drive. This is where your saved clips belong.
4. Launch **nvidia-mem-replay** from the Start menu. To change the drive or RAM limit, choose an unused letter and a limit in **Settings**, then click **Apply and restart**. The default limit is **8192 MB**; leave enough RAM for your game and Windows.
5. Switch **Instant Replay** off and on in **Alt+Z**, then save clips as usual.

## While you play

- **Close the window** to keep it running in the tray. Click the tray icon to reopen it. If the tray is unavailable, closing quits the app.
- **Stop and restore** stops RAM recording and restores NVIDIA’s original temporary folder.
- **Quit** in the tray closes the app and restores that folder.

**Save any clips you want before stopping, quitting, restarting, or shutting down your PC.** The temporary RAM buffer disappears; clips already saved to Gallery stay on disk.

## Quick fixes

- If recording doesn’t start, toggle Instant Replay off and on in **Alt+Z**.
- If memory runs low, lower the RAM limit in **Settings**.
- If shutdown fails, follow the message in the app and choose **Retry shutdown**.

Windows can still page RAM to disk, so this does not guarantee zero disk writes.

## Credits

Built on the work of these projects and their contributors:

- **[WinFsp](https://github.com/winfsp/winfsp)** — the Windows filesystem driver that makes the RAM drive possible.
- **[WinFsp-MemFs-Extended](https://github.com/Ceiridge/WinFsp-MemFs-Extended)** by **Ceiridge** — the dynamically allocated RAM filesystem behind this app.
