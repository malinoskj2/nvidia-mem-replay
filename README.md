# nvidia-mem-replay

![Windows x64](https://img.shields.io/badge/Windows-x64-0078D4)
![NVIDIA Instant Replay](https://img.shields.io/badge/NVIDIA-Instant_Replay-76B900)
[![GPL-3.0 license](https://img.shields.io/badge/license-GPL--3.0-555555)](LICENSE)

**Use RAM for NVIDIA Instant Replay’s temporary recordings.**
Your saved clips still go to your usual Gallery folder.

You need **64-bit Windows**, an **NVIDIA GPU**, and **Instant Replay** working in the NVIDIA overlay.

## Get started

1. Run **`nvidia-mem-replay-setup-x64.exe`** from the Windows download. WinFsp is included; Windows may ask for admin access or a reboot. The last page offers to launch the app right away.
2. Open the NVIDIA overlay with **Alt+Z**. In **Settings → Files and disk space**, set **Temporary files** to a folder if you haven’t already.
3. Keep **Gallery** on your SSD or hard drive. This is where your saved clips belong.
4. Launch **nvidia-mem-replay** from the Start menu while the NVIDIA App (or GeForce Experience) is running. The app switches NVIDIA's temporary-files location to RAM storage through NVIDIA's own ShadowPlay interface, the same way the overlay does. The RAM volume is invisible: it has no drive letter and does not appear in Explorer, This PC or file dialogs. To change the RAM limit, set it in **Settings**, then click **Apply and restart**. The default limit is **8192 MB**; leave enough RAM for your game and Windows.
5. Save clips as usual. If Instant Replay is recording, the app switches it off and on for you (by pressing its own Alt+Z hotkey) so it records into the new location; if that is not possible, the window says so and you can toggle it in **Alt+Z** yourself.

## While you play

- **Close the window** to keep it running in the tray. Click the tray icon to reopen it; right-click it for **Stop and restore** and **Quit**. If the tray is unavailable, closing quits the app.
- **Stop and restore** (Settings tab or tray menu) stops RAM recording and restores NVIDIA’s original temporary folder.
- **Start with Windows** (Settings tab) starts the app hidden in the tray at sign-in. It waits for the NVIDIA App to come up before redirecting, so the order the two start in does not matter.
- If the NVIDIA App or its overlay restarts while the app is running, NVIDIA resets its temporary folder; the app notices within a few seconds and redirects it to RAM again.

**Save any clips you want before stopping, quitting, restarting, or shutting down your PC.** The temporary RAM buffer disappears; clips already saved to Gallery stay on disk.

## Quick fixes

- If recording doesn’t start, toggle Instant Replay off and on in **Alt+Z**.
- If the app reports that ShadowPlay could not be reached, open the NVIDIA overlay once with **Alt+Z** so the NVIDIA App’s background services are running, then click **Retry / start**.
- If memory runs low, lower the RAM limit in **Settings**.
- If shutdown fails, follow the message in the app and choose **Retry shutdown**.

Windows can still page RAM to disk, so this does not guarantee zero disk writes.

## Credits

Built on the work of these projects and their contributors:

- **[WinFsp](https://github.com/winfsp/winfsp)** — the Windows filesystem driver that makes the RAM drive possible.
- **[WinFsp-MemFs-Extended](https://github.com/Ceiridge/WinFsp-MemFs-Extended)** by **Ceiridge** — the dynamically allocated RAM filesystem behind this app.
