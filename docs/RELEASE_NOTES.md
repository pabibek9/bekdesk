## bekdesk 0.1.14 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **bekdesk for Linux (preview):** `bekdesk-Linux-x86_64.rpm` for Fedora (`sudo dnf install ./bekdesk-Linux-x86_64.rpm`) and `bekdesk-Linux-x86_64.AppImage` for any distribution. It finds your other computers, pairs with the same code screen, and does **Share Input** (one keyboard and mouse across Windows, Mac and Linux), clipboard sync and file sending. Deskflow comes from your package manager if installed, otherwise bekdesk installs Deskflow's official Flatpak for your user (no password). Extend, Duplicate and tablets aren't available on Linux yet; those buttons are greyed out with the reason. See INSTALL.md for the Fedora firewall line and Wayland permission.
- **One bekdesk at a time:** opening bekdesk again shows the running window instead of starting a second copy (on Linux, GNOME has no tray icon).
- bekdesk starts even where the desktop has no tray area.
- When an engine can't be set up, the message now says why instead of always guessing "no internet".

## bekdesk 0.1.13 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Arrange your own screens in Manage Screens:** drag a second screen of this PC (e.g. an HDMI monitor) next to, above or below the main one, like in Windows display settings; bekdesk applies it to Windows and snaps it exactly to the edge. The main screen stays put (drag it to move the whole PC).
- **See and manage every screen:** each of this PC's screens shows how it's connected (Built-in, HDMI, DisplayPort, USB-C, VGA, Wireless) and the monitor's name on hover. Right-click a screen to change its **resolution and refresh rate**, **rotate** it, **turn it off**, make it the main display, identify screens, or open Windows display settings. Like Windows, a resolution or rotation change is undone after 15 seconds unless you press **Keep**, so a monitor that goes black fixes itself.
- **Identify shows numbers, not names:** every screen gets 1, 2, 3…; a tablet you extend to is the next number after this PC's own screens, and Manage Screens shows the same number on it.
- **Copy diagnostics now lists every display Windows has active** (position, size, refresh, which is main, and which is bekdesk's virtual screen), so an unexpected extra display can be pinned down.
- Fix: snapping a device above or below another in Manage Screens used the wrong height.

## bekdesk 0.1.12 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Fix: the tablet couldn't connect with Extend in 0.1.11** ("lost connection" in Moonlight). When a device connects, Sunshine starts bekdesk to switch the extra screen on, and Windows refused with "access denied": bekdesk keeps its engines in a group that closes them when bekdesk closes, and that group didn't allow Sunshine to start the command the way it does. The group now allows it; engines still close with bekdesk.

## bekdesk 0.1.11 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **No more invisible second screen.** bekdesk's virtual screen now exists only while a tablet or computer is actually showing it: it switches on the moment a device connects and off when it disconnects. If anything else turns it on while nothing is connected (Win+P "Extend", plugging in an HDMI monitor, bekdesk having closed mid-stream), bekdesk switches it off again within a couple of seconds, so windows can't get lost on a screen nobody sees. Only bekdesk's own virtual screen is touched; your laptop screen and HDMI monitors are left alone.
- **Extend stays on when the tablet disconnects.** It no longer switches itself Off: reconnect from Moonlight any time and the extra screen comes back where you put it.
- **The extra screen matches the device:** when the tablet asks for a resolution (e.g. Moonlight set to Native), the virtual screen uses exactly that size.
- **Manage Screens shows the real state:** a tablet that isn't connected is greyed out as "Not connected" instead of showing "Extend".

## bekdesk 0.1.10 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Fix: dragging the tablet below this PC in Manage Screens made the tablet the main display**, so the desktop icons and taskbar jumped to the tablet and this PC's screen was left empty. bekdesk now checks every couple of seconds (and right after you move a screen) and gives the main-display role back to this computer's own screen. If bekdesk starts with the tablet already the main display, it fixes that too.

## bekdesk 0.1.9 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Fix: one tablet listed under several names.** Each time the same tablet paired again, Sunshine added another entry and kept the old ones. bekdesk now keeps only the newest entry per device and removes the rest automatically (also cleaning up existing duplicates).
- **Fix: Remove didn't work.** It asked "are you sure?" with a pop-up that doesn't reliably appear inside the app. Remove (and Block in Settings) now asks by changing the button to "Press again to remove"; press it a second time within 4 seconds.
- **Fix: several "New tablet or phone" cards for one tablet.** When Moonlight retries, only its newest pairing request is kept; older ones from the same device are cancelled.
- **Clear steps for "not authorized (401)" in Moonlight:** this happens when Moonlight still remembers a pairing this PC no longer has. In Moonlight, long-press the PC → **Delete PC**, wait for it to reappear, tap it and pair again with the PIN.

- **Tablets and phones are found for real:** no more ready-made "Moonlight tablet" card. bekdesk now listens for Moonlight in the background (it can't show your screen until you pick Extend or Duplicate). When a tablet or phone taps this computer in Moonlight, a **New tablet or phone** card appears with its address: type the PIN it shows, give it a name, and press **Pair**. Pairing requests also show up when the card already says paired.
- **Name your devices:** the name you type is what bekdesk shows everywhere. Android doesn't tell other devices its name, so bekdesk asks.
- **Remove a paired device** from its card. If you removed this computer in Moonlight, remove the device here too and pair again.
- **Fix "A pairing session with this uniqueid already exists (409)":** a pairing request Moonlight abandoned used to block every new try. Press **Decline**, or wait two minutes and bekdesk clears it.
- **Honest status:** a paired device that isn't streaming now shows **Not connected** instead of "Waiting for Moonlight".
- Several tablets or phones can be paired; one of them streams at a time.

## bekdesk 0.1.7 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Fix: the tablet kept showing a copy of the screen while bekdesk said Off**, and updates failed with "the process cannot access the file because it is being used by another process". A screen-sharing engine started by an older bekdesk was still running in the background (closing or uninstalling bekdesk didn't stop it). bekdesk now stops leftover engines when it starts, and on Windows every engine it starts now closes automatically when bekdesk closes or crashes.

- **Fix: Moonlight asking for the PIN again after it was already paired.** Sunshine's certificate and Moonlight's identity were stored inside the engine folders, which updates and reinstalls replace; both now live in bekdesk's own data (existing ones are carried over where they still exist, so you may have to pair the tablet one last time).
- **Sharper picture:** the virtual screen now offers 16:10 and common tablet/laptop sizes (1280×800, 1440×900, 1680×1050, 1920×1200, 2000×1200, 2560×1600, 2880×1800, 2048×1536) plus the receiver's exact size, instead of falling back to a scaled 1920×1080. If the driver was installed by an older bekdesk, Windows asks once to add the sizes. The tablet card now explains how to set Moonlight to **Native** resolution (Moonlight on Android streams 720p unless you change it, and only the tablet can change that).
- **Copy and paste no longer fight:** Deskflow's own clipboard is now really turned off (the old setting was ignored by Deskflow 1.27), so only bekdesk syncs the clipboard.
- "Allow Once" is now called **Only this time**, because it asks again after bekdesk restarts; choose **Always Allow** to be asked only once.

- **Reconnect button:** every connected device (and the tablet) has a **Reconnect** button that disconnects and connects again in one click. The Share Input card now says plainly that your keyboard types on whichever computer the mouse is on.

- **Bring the mouse home:** press **Ctrl+Alt+Home** while sharing input to jump the pointer back to this computer.
- **The pointer crosses at the right height:** if one laptop sits higher or lower than the other in Manage Screens, the mouse now crosses only along the part of the edge where they overlap, instead of jumping.
- **Extend starts faster:** bekdesk remembers the virtual screen, so the screen-sharing engine starts once instead of twice.
- **Clearer errors:** "can't reach" now tells you what to check (bekdesk open on both, same Wi-Fi, Windows Firewall on Private networks); on a Mac, Share Input tells you exactly which permission to allow.
- **Copy diagnostics** (Connections → Log): copies a full status report with recent engine logs, to paste when asking for help.

- **Your PC stays the main screen:** if Windows makes the new virtual screen the main one when a tablet or laptop extends, bekdesk switches it back. In Manage Screens, **right-click** one of your screens to make it the main display, identify screens, or open Windows display settings.
- **Sound only where you want it:** by default sound now plays only on this computer. Each Extend/Duplicate device has **Also play sound on …** to add it there too.
- **The tablet switches itself off** a few seconds after it disconnects, instead of staying on "Extend".
- **Tablets and phones:** the card only appears once you add one (**+ Add a tablet or phone**) or after one has paired, and shows the device's name. Phones work the same way (Moonlight app).
- **Identify screens** now also shows on the extended tablet/laptop screen (with its name) and on every connected computer.
- **Sharper text for laptop receivers:** about 1.5× more bitrate.
- **Scan** button; bekdesk now checks for devices when you open it or bring it back, about once a minute in the background instead of every 16 seconds.
- **Connections** lists the tablet/phone too, and empty sections explain what will appear there.
- **Settings → Devices** lists every computer bekdesk has seen, with **Ask to connect**, **Forget** and **Block**.
- Better laptop, desktop and tablet icons; GitHub link under About.

- **Mouse speed per device (Windows):** each Extend or Share Input device, and the tablet, has a **Mouse speed** slider. bekdesk switches Windows' pointer speed while the mouse is on that device and puts yours back when it comes home (or when bekdesk quits), so a small tablet or a high-resolution laptop no longer feels slow.
- **Smoother app:** spring animations when opening the app and switching pages, a sliding menu highlight, a "connected" pop on the device card, and smoother dialogs, menus and messages.

- **Frame rate with headroom:** laptop receivers now stream at about 90% of their screen's refresh rate (108 fps on 120 Hz, 129 on 144 Hz, 60 on 60 Hz, at most 144), with the bitrate scaled to match, so frames don't pile up and lag. The tablet card says which frame rate to pick in Moonlight.

Everything from 0.1.5 and 0.1.6 (rebuilt Nearby Devices and Manage Screens, your address and Connect by address, Identify screens, automatic downloads, low-latency streaming) is included.

### Still preview
The Extend fix and tablet status are based on Sunshine's documentation and source and have not been tried on real hardware yet. Tested on one Windows PC and in CI only; input sharing and screen streaming have not yet been tested between two physical machines.
See https://github.com/pabibek9/bekdesk/blob/main/docs/STATUS.md. Installers are not code-signed: SmartScreen/Gatekeeper will warn once.

Verify downloads against `SHA256SUMS.txt`. Licenses: `LICENSE`, `THIRD_PARTY.md`, `THIRD_PARTY_LICENSES.html`.
