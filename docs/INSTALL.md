# Installing and pairing bekdesk

## 1. Install on each computer

**Windows 10/11:** run `bekdesk-Windows-x64-Setup.exe`. It installs for your user only (no admin).
SmartScreen may say "Windows protected your PC" because the installer is not code-signed yet:
choose *More info → Run anyway*. On first start, allow bekdesk through the Windows Firewall on
**private** networks when Windows asks.

**macOS 11+:** open `bekdesk-macOS-universal.dmg` and drag bekdesk to Applications. The app is not
notarized yet: the first time, right-click it → *Open* → *Open*. Allow *Local Network* access when asked.
macOS will also ask, the first time each feature runs, for:
- *Accessibility* and *Input Monitoring* for `deskflow-core` (Share Input),
- *Screen Recording* for `sunshine` (Extend/Mirror from this Mac).

Enable these in System Settings → Privacy & Security, then pick the mode again.

All computers must be on the same network (same Wi-Fi/LAN, not a guest network that isolates clients).

## 2. Pair

1. Open bekdesk on both computers. Each appears under **Nearby Devices** on the other within a few seconds.
2. Click the other computer and choose a mode.
3. The other computer shows **"<name> wants to connect"** with a 6-digit code. Check that the code
   matches the one shown on your computer, then pick:
   - **Always Allow** — remember this device; it reconnects automatically after sleep or restart.
   - **Allow Once** — until bekdesk restarts.
   - **Decline**, or **Block** to ignore it from now on.
4. bekdesk downloads the engines it needs (Deskflow ~18–29 MB, Sunshine ~40 MB, Moonlight ~37–78 MB)
   by itself shortly after it starts; progress shows at the top of Nearby Devices.

Revoke or limit a device any time in **Settings → Trusted devices** (control, clipboard, files, screen).

## 3. Arrange

Open **Workspace** and drag the computers to match your desk. With Share Input, the pointer crosses
where the boxes touch. For Extend, the virtual monitor is placed on the side where you put the device.

## 4. Move files

- With **Share Input** on: copy files (Ctrl+C / Cmd+C) on one computer, then paste on the other. Files
  up to 2 GB transfer automatically; text and images sync instantly.
- Any paired device: drag files from Explorer/Finder onto the device's card in bekdesk.
- Received files are in `Downloads/bekdesk/`.

## Linux (preview: Share Input, clipboard and files)

**Fedora:** download `bekdesk-Linux-x86_64.rpm` and install it:

```bash
sudo dnf install ./bekdesk-Linux-x86_64.rpm
```

Then open **bekdesk** from the app menu. **Any distribution:** download `bekdesk-Linux-x86_64.AppImage`, then
`chmod +x bekdesk-Linux-x86_64.AppImage` and run it (Fedora may need `sudo dnf install fuse-libs` once).

- The first time you choose **Share Input**, bekdesk installs Deskflow for your user from its official Flatpak (no
  password; it downloads the KDE runtime once, a few hundred MB). If Deskflow is already installed from your
  package manager, that one is used.
- On Wayland (Fedora's default) GNOME asks once to **allow remote control** of the desktop; allow it, or the
  mouse can't move into the Linux computer.
- GNOME shows no tray icon by default. Closing the window keeps bekdesk running; open it again from the app menu.
- Extend, Duplicate and tablets need the screen-sharing engines, which aren't available for Linux yet; those
  buttons are greyed out with the reason.

**Firewall (Fedora):** allow bekdesk's port on your home network:

```bash
sudo firewall-cmd --permanent --add-port=47290/tcp --add-service=mdns && sudo firewall-cmd --reload
```

**Testing two copies on one Linux machine** (no second computer needed):

```bash
BEKDESK_DATA=/tmp/bekdesk-b BEKDESK_PORT=47291 bekdesk
```

## 5. Android tablet

bekdesk for Android isn't available yet; the tablet uses **Moonlight** as its screen receiver.

1. Install Moonlight from the Play Store, F-Droid, or https://github.com/moonlight-stream/moonlight-android/releases.
2. On the computer, in **Nearby Devices → Tablets & phones**, choose *Mirror this screen*, or on
   Windows *Extend to tablet* and type the tablet's resolution (e.g. `2560x1600`).
3. On the tablet, Moonlight lists the computer automatically. Tap it, note the 4-digit PIN, enter it in
   bekdesk and press **Pair**. Then tap **Desktop**. Touch input controls the computer.

## Uninstall

Windows: *Settings → Apps → bekdesk → Uninstall*. macOS: drag bekdesk to the Trash.
App data (identity, trusted devices, downloaded engines) lives in
`%APPDATA%\io.github.pabibek9.bekdesk` or `~/Library/Application Support/io.github.pabibek9.bekdesk`;
delete it to forget all pairings. If Extend was used on Windows, the virtual display driver stays
installed: remove "Virtual Display Driver" under Device Manager → Display adapters.
