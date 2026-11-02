## bekdesk 0.1.28 — preview

### Polish
- **Manage Screens:** monitors have a light rim so overlapping ones stay distinct; hovering or dragging a device lifts and highlights it; device names sit on a soft label above every monitor. The screens area and the kurtha girl now share the window height, so the whole illustration shows without scrolling at any normal window size.
- **Settings:** the controls keep clear of the palace artwork, which fades in on the right only.
- **Connections:** clearer empty-state text and calmer spacing.
- **Sidebar:** the update status is one compact line: "Update check failed · Retry", "Check for updates", or the Update button.

## bekdesk 0.1.27 — preview

Everything below from 0.1.23 to 0.1.26 in one release: lock screen, sound both ways, microphone, camera, shared files, right-click Send with bekdesk, Wake, Lock all, battery and Ctrl+Alt+Arrow switching.

## bekdesk 0.1.26

### Handy extras
- **Ctrl+Alt+Arrow keys** jump the mouse straight to the computer on that side (Ctrl+Alt+Home brings it back).
- **Lock all** (bottom of the sidebar): locks every computer you're connected with, and this one.
- **Wake:** a sleeping or switched-off paired computer gets a **Wake** button. It needs Wake-on-LAN allowed in that computer's network settings/BIOS, and bekdesk learns its address while both are on.
- **Battery** level of paired laptops shows on their card, with a "low" warning.
- Game controllers already work on a computer you show with Extend or Duplicate.

### Camera (first version)
- The camera's owner ticks **Let <name> use this PC's camera**; on a Windows PC the other computer ticks **Use <name>'s camera here**, and it appears as **bekdesk camera** in Zoom, Teams, browsers and other camera apps.
- Sending works from Windows and from Fedora (Fedora uses GStreamer, which GNOME already includes).
- Receiving needs the "bekdesk camera" part installed by Settings → Lock screen, camera and admin prompts → Turn on.

## bekdesk 0.1.25 — preview

### Files
- **Shared files** on Connections: everything other computers sent you, newest first, with picture previews. Open it, show it in its folder, or **Send on** to other computers.
- **Right-click → Send with bekdesk:** on Windows under **Send to** ("Show more options" on Windows 11); on Fedora in GNOME Files under **Scripts**. Pick one or more computers and the files go to all of them.
- "Open folder" buttons now work on Linux.

## bekdesk 0.1.24 — preview

### Sound in every direction
- Windows can now send its sound too (before, only Linux could).
- On any computer connected with Share Input (either way), under **More**:
  - **Play <name>'s sound on this PC**: hear that computer here. When you control a Linux computer, its own speakers go quiet.
  - **Also play this PC's sound on <name>'s speakers**: tick it on several computers to play this PC's sound on all of them at once. They aren't perfectly in sync yet; expect a slight echo in the same room.
- Fixed a crash when ticking a sound box (the setting ran where it couldn't start the sound).

### Microphone sharing
- The microphone's owner ticks **Let <name> use this PC's microphone**; then the other computer ticks **Use <name>'s microphone here**.
- Fedora: pick **bekdesk-microphone** in your app (bekdesk creates it while it's in use).
- Windows: install the free **VB-CABLE** (vb-audio.com/Cable) once, then pick **CABLE Output** as the microphone in your app.
- Not yet from or to a Mac.

## bekdesk 0.1.23 — preview

### New: keyboard and mouse on the Windows lock screen
- **Settings → Lock screen and admin prompts → Turn on** (or the box on the welcome screen). Windows asks for admin permission once. It also sets up the extra-screen driver in the same step, so neither prompt surprises you later.
- While another computer controls this PC, its keyboard and mouse then also work on the lock screen (type your password from the other computer) and on Windows admin (UAC) prompts.
- How it works: a small "bekdesk lock screen helper" service runs Deskflow's client with system rights, only while Share Input is on. It lives in `C:\ProgramData\bekdesk`, which only administrators can change, and only reads settings from a folder only you can write to. It uses Deskflow 1.27, which includes the fix for the earlier Deskflow service security issue (CVE-2026-41477).
- Before uninstalling bekdesk, turn this off in Settings, so the helper is removed too.
- Not possible on Fedora: GNOME doesn't let any app control its login screen.

## bekdesk 0.1.22 — preview

### Fixes
- **The Allow prompt always appears:** when another computer asks to connect, bekdesk now opens its window (even from the tray) and flashes it, instead of the request silently timing out. If a computer had forgotten the other one (reinstalled, or Forget pressed), it asks again instead of showing an error.
- **Manage Screens stays where you put it:** a device lands exactly where you drop it and only clicks onto an edge when it's already within a few pixels. The other devices no longer shift or shrink when you move one.
- **Tablets and phones listed separately:** each paired device has its own row, marked "Showing now" or "Not showing".
- **Scrolling:** Settings has a scroll speed and a reverse-direction option for when another computer controls this one. Two-finger scrolling is passed on; pinch and swipe gestures can't be (Deskflow doesn't support them).
- **Extend:** when Windows rejects a size for the extra screen, bekdesk now uses the closest size the driver offers instead of failing with "-1". Picture problems on the receiving computer now say whether it's the firewall or the video decoder.
- Starting a mode no longer waits for unrelated downloads.

### New
- **Welcome screen** on first start: name this computer, start at sign-in, and what Windows, Fedora or macOS will ask to allow.
- **Getting ready screen** while parts download, with the Dhaka-topi boy and friendly messages; the app waits until it's ready.
- **Light / Dark / Auto** in Settings, plus a quick switch at the bottom of the sidebar.
- "Easy connection, easy life" on the Connections banner.

## bekdesk 0.1.21 — preview

### Changes
- **New look with Nepal artwork:** icons in the sidebar with Boudhanath above your profile; a boy in a Dhaka topi beside "Can't see a computer?" on a Himalayan dawn; Everest across Manage Screens with a girl in a kurtha and her pagoda temple just below it; small handwritten captions beside both; Pashupatinath on Connections and Janaki Mandir on Settings. Mode buttons have icons, and Settings groups each section in its own card with clearer permission descriptions. The illustrations never take clicks, so dragging screens works as before; the logo is unchanged.
- The Share Input hint no longer mentions the edge drop box, which 0.1.20 switched off.

## bekdesk 0.1.20 — preview

### Changes
- **Crash fixes (cause not confirmed):** the drop-zone window at the screen edge is switched off, since it was the newest thing that runs while sharing; a problem in a background task no longer closes the whole app; any panic is written to `logs/crash.log` in the data folder (Connections -> Open logs).
- **Sound:** the Linux packages now require `pulseaudio-utils`, which sound sharing needs. Windows and macOS still cannot send sound.

## bekdesk 0.1.19 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Hear the other computer here:** while this PC shares its keyboard and mouse with a Linux computer, open **More** on that computer's card and tick **Play <name>'s sound on this PC**. Its sound then comes out of this PC's speakers and its own speakers go quiet; they come back when you untick it or stop sharing. Both computers need 0.1.19. The Linux computer needs `pactl`, `parec` and `pacat` (the pulseaudio-utils package, installed on Fedora and Ubuntu desktops). Sending sound from Windows or a Mac isn't available yet.
- **No more surprise lock screens while you share:** a computer being controlled from another keyboard now stays awake, so its screen doesn't blank and lock just because you were working on the other computer for a while. You can still lock it yourself, and you can turn this off per computer under **More** on its card. bekdesk never unlocks a computer or types its password for you: if it's already locked, unlock it there.

## bekdesk 0.1.18 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **See your version:** the bottom of the sidebar shows which bekdesk you're using.
- **Updates:** bekdesk checks for a new version a minute after it starts and every six hours, or when you press **Check for updates**. When one is out, press **Update to …**: it downloads the right file for your computer, checks it against the release's checksums, installs it and reopens bekdesk (Windows and Linux; on a Mac the new disk image opens so you can drag it to Applications).
- **Drag files and images to another computer:** while this computer shares its keyboard and mouse, a small blue box sits near the screen edge that faces each connected computer. Drag files or images onto it and they're sent there and put on that computer's clipboard, so you just press Ctrl+V (Cmd+V) where you want them.

## bekdesk 0.1.17 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Scroll Lock warning (Windows):** while this PC shares its keyboard and mouse, Scroll Lock keeps the mouse from leaving the screen. The device card now says "Scroll Lock is on, so the mouse stays on this PC" with a **Turn off Scroll Lock** button.
- **Every card says what's wrong and what to do:** in plain words, from what the connection actually reports, e.g. the other computer connected but didn't finish setting up (allow the remote-control prompt on Linux), connected under a name this PC doesn't expect, hasn't connected yet, wasn't allowed to be controlled, the extra screen couldn't switch on, the picture dropped (Wi-Fi), a tablet isn't paired any more (401), or a file didn't send. "Can't reach" now names the exact firewall step for the other computer's system (Windows, macOS or Fedora). A **Details** link on the card shows the latest log lines for that connection.
- **Locked or asleep:** a connected computer that is locked shows **Locked** and "fedora is locked. Unlock it on that computer to continue." (Windows and Linux report this; macOS doesn't yet). A known computer that's off the network shows **Asleep or off** and that it reconnects by itself.
- **Simpler screens:** each card shows one message and one button at a time; mouse speed, sound, Reconnect (when nothing's wrong), Forget, and the tablet's paired devices and tips are under **More**. Shorter mode labels ("This PC's keyboard & mouse", "Extra screen", "Same screen", "Not connected"); "Can't see a computer?" folds the address box away; Connections has a short Troubleshooting box with **Copy diagnostics** first.
- **Screen numbers:** only this PC's screens are numbered in Manage Screens, so another device no longer shows a second "1 Main".
- **Phones and tablets as keyboard/mouse targets:** researched, not built yet; see docs/ANDROID_INPUT.md.

## bekdesk 0.1.16 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Quit bekdesk** from the sidebar (press twice). It stops all sharing and closes bekdesk completely, which matters on Linux desktops that have no tray icon.
- **Share Input explains what's wrong** right on the device's card instead of silently not working: the other computer hasn't connected yet, it connected under a name this computer doesn't expect, or (on Linux) it hasn't been allowed to be controlled yet. Each comes with what to do; the advice disappears once it works.

## bekdesk 0.1.15 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Choose whose keyboard and mouse are shared:** **Share Input** now says "Use this PC's keyboard & mouse": the computer where you press it is the one whose keyboard and mouse move to the others. When another computer is controlling this one, its card says "Controlled by …" and offers **Use this PC's keyboard & mouse instead**, which switches the direction in one click.
- **Linux setup that doesn't look stuck:** keyboard & mouse sharing uses what's already installed; on Fedora 44 it installs a small package after your normal password prompt; otherwise it says plainly that the first download is large and can take a while.
- **Simpler:** **Forget this computer** right on its card in Nearby Devices; permissions in Settings are folded away (everything is allowed by default); setup messages describe what's being set up instead of naming components; tablets see this computer as "bekdesk <name>".
- **Debian/Ubuntu package:** releases now include `bekdesk-Linux-amd64.deb` as well as the Fedora `.rpm` and the `.AppImage`.

## bekdesk 0.1.14 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **bekdesk for Linux (preview):** `bekdesk-Linux-x86_64.rpm` for Fedora (`sudo dnf install ./bekdesk-Linux-x86_64.rpm`) and `bekdesk-Linux-x86_64.AppImage` for any distribution. It finds your other computers, pairs with the same code screen, and does **Share Input** (one keyboard and mouse across Windows, Mac and Linux), clipboard sync and file sending. It can also be a **screen for your Windows PC**: on the Windows PC choose **Extend** or **Duplicate** for the Linux computer, and bekdesk shows it there with Moonlight's official AppImage (downloaded automatically, no password). Deskflow comes from your package manager if installed, otherwise bekdesk installs Deskflow's official Flatpak for your user (no password). Sharing the Linux computer's own screen (to a PC or tablet) isn't available yet; those buttons are greyed out with the reason. See INSTALL.md for the Fedora firewall line and Wayland permission.
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
