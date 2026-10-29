## bekdesk 0.1.4 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Manage Screens redesigned:** this PC sits in the centre; every computer and the tablet is drawn as a real device (desktop monitor with stand, laptop with keyboard, tablet), each detected monitor in its own numbered frame with the main one marked. Drag anything anywhere; it snaps to edges and the view re-centres.
- Laptops are detected automatically (built-in battery) and shown as laptops on other computers.
- Extend to tablet now places the virtual monitor on the side where you put the tablet in Manage Screens.
- Fix: Extend (and Extend to tablet) failed with "display mode … rejected (-1)" because the virtual monitor could not be switched on. It is now activated in extend mode first (verified on Windows 11 with Virtual Display Driver 25.7.23).
- The Windows installer file now shows the bekdesk logo (0.1.2).
- New blue app logo, readable on dark and light Start menus and taskbars.
- Nearby Devices no longer lists this computer itself, and shows step-by-step instructions until another computer is found.
- Devices that stopped answering (closed without saying goodbye) disappear from the list within ~20 seconds instead of showing as "Available".
- "Workspace" is now **Manage Screens**, "Activity" is now **Connections**, each with a one-line explanation.
- Clearer Android tablet instructions (the tablet uses the free Moonlight app as its screen).

### Still preview
Tested on one Windows PC and in CI only; input sharing and screen streaming have not yet been tested between two physical machines.
See https://github.com/pabibek9/bekdesk/blob/main/docs/STATUS.md. Installers are not code-signed: SmartScreen/Gatekeeper will warn once.

Verify downloads against `SHA256SUMS.txt`. Licenses: `LICENSE`, `THIRD_PARTY.md`, `THIRD_PARTY_LICENSES.html`.
