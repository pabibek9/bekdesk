## bekdesk 0.1.5 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Fix: Extend showed a copy of the main screen.** Sunshine needs the virtual monitor's device ID, not its `\\.\DISPLAYn` name, and otherwise falls back to the main display. bekdesk now reads the ID from Sunshine's own startup log and restarts it on the right monitor. If the virtual monitor can't be found it now says so instead of silently duplicating.
- **Nearby Devices rebuilt:** every device, including the Android tablet, is a card with its connection status (Ready / Connecting / Connected · Extend) and the Extend / Duplicate / Share Input / Off buttons always visible. "Mirror" is now called **Duplicate**.
- **Tablet connection is tracked:** the card shows Waiting for the tablet, Paired, or Connected, with the setup steps and a PIN box that pairs by itself after 4 digits.
- **Clicks no longer get lost:** the window used to redraw every second, which could swallow a click mid-press. Screens now update in place, with animations.
- **Manage Screens:** the virtual monitor no longer shows up as an extra screen of this PC (it was the "third screen"); the monitor list stays live when screens change; names and status sit under each device without overlapping; every screen has a number and **Identify screens** flashes that number on the real screens. Moving a device while Extend is on moves the virtual monitor to match.
- **Everything bekdesk needs downloads by itself** a few seconds after it starts (progress at the top of Nearby Devices), and retries if you were offline, instead of waiting for the first connection.

### Still preview
The Extend fix and tablet status are based on Sunshine's documentation and source and have not been tried on real hardware yet. Tested on one Windows PC and in CI only; input sharing and screen streaming have not yet been tested between two physical machines.
See https://github.com/pabibek9/bekdesk/blob/main/docs/STATUS.md. Installers are not code-signed: SmartScreen/Gatekeeper will warn once.

Verify downloads against `SHA256SUMS.txt`. Licenses: `LICENSE`, `THIRD_PARTY.md`, `THIRD_PARTY_LICENSES.html`.
