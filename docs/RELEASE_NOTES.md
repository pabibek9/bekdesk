## bekdesk 0.1.6 — preview

**Install it yourself by double-clicking the installer** (not from inside another app), so Windows registers it normally with its icon.

### Changes
- **Fix: the tablet kept showing a copy of the screen while bekdesk said Off**, and updates failed with "the process cannot access the file because it is being used by another process". A screen-sharing engine started by an older bekdesk was still running in the background (closing or uninstalling bekdesk didn't stop it). bekdesk now stops leftover engines when it starts, and on Windows every engine it starts now closes automatically when bekdesk closes or crashes.

Everything from 0.1.5 (rebuilt Nearby Devices and Manage Screens, your address and Connect by address, Identify screens, automatic downloads, low-latency streaming) is included.

### Still preview
The Extend fix and tablet status are based on Sunshine's documentation and source and have not been tried on real hardware yet. Tested on one Windows PC and in CI only; input sharing and screen streaming have not yet been tested between two physical machines.
See https://github.com/pabibek9/bekdesk/blob/main/docs/STATUS.md. Installers are not code-signed: SmartScreen/Gatekeeper will warn once.

Verify downloads against `SHA256SUMS.txt`. Licenses: `LICENSE`, `THIRD_PARTY.md`, `THIRD_PARTY_LICENSES.html`.
