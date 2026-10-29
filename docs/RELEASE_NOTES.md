## bekdesk 0.1.0 — preview

First release. **Preview quality: tested on one Windows PC and in CI only, not yet across two
physical machines.** Read the feature status before relying on it:
https://github.com/pabibek9/bekdesk/blob/main/docs/STATUS.md

### Downloads

- `bekdesk-Windows-x64-Setup.exe` — Windows 10/11 x64, per-user install
- `bekdesk-macOS-universal.dmg` — macOS 11+, Apple Silicon and Intel
- Android: use Moonlight for Android as the receiver (see INSTALL.md)

**Not code-signed or notarized.** Windows SmartScreen and macOS Gatekeeper will warn on first launch.
Verify downloads against `SHA256SUMS.txt`.

### What's in it

- Automatic discovery of bekdesk devices on the LAN; approval prompt with matching code; trusted devices with per-capability permissions.
- Share Input (Deskflow) with bekdesk-managed clipboard and copy/paste of files and folders.
- Mirror and Extend (Sunshine + Moonlight; virtual monitor on Windows hosts).
- Workspace layout, Activity view, tray, start at login.

### Licenses

bekdesk is GPL-3.0-or-later. Engines are downloaded from their official releases at first use; see
`THIRD_PARTY.md`. `THIRD_PARTY_LICENSES.html` lists the Rust crates linked into the app.
