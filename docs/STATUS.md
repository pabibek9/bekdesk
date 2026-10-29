# Feature status — 0.1.0

Legend: **Tested** = exercised on real hardware/binaries. **Built** = implemented and compiles, not
yet run end to end across two physical machines. **Not available** = not implemented.

Test environment so far: one Windows 11 PC (two bekdesk instances side by side) and GitHub Actions
Windows/macOS runners. No second physical computer, Mac or Android tablet has been used yet.

| Feature | Status | Evidence / notes |
|---|---|---|
| LAN discovery (mDNS) | **Tested** | Two real instances discovered each other with addresses and monitor layouts. |
| Device identity + mutual TLS 1.3 | **Tested** | `tests/e2e.rs`: real TLS on loopback; impostor advertising another fingerprint is refused. |
| Pairing approval: Allow Once / Always / Decline / Block | **Tested** | `tests/e2e.rs`; matching code on both sides. UI dialog rendered with real data. |
| Per-device capability permissions + revocation | **Tested** | `tests/e2e.rs` (files capability revoked → refused). |
| File and folder transfer | **Tested** | `tests/e2e.rs`: nested folder + 3 MB file byte-identical; path sanitising unit-tested. |
| Drag files onto a device card | Built | Uses Tauri's native drop event. |
| Copy files on one computer, paste on another | Built | Clipboard watcher → transfer → peer clipboard set to received files. Needs two machines to test. |
| Text/image clipboard sync | Built | Via clipboard-rs; Deskflow's own clipboard is disabled to avoid fights. Rich text/HTML not synced. |
| Share Input (Windows↔Windows, Windows↔macOS) | Built | Deskflow 1.27 config format verified against the real binary (server parses, listens on 127.0.0.1 only, client handshake starts). Full cross-machine control not yet tested. |
| Workspace layout → screen edges | Built | Layout → Deskflow links unit-tested. |
| Mirror Display (Windows/macOS host → Windows/macOS viewer) | Built | Sunshine config + credentials and Moonlight↔Sunshine PIN pairing automation **tested locally against the real binaries**; actual streaming between two machines not yet tested. |
| Extend Display, Windows host | Built | Virtual Display Driver install (UAC) + attach/detach via Win32. Not yet run: the driver has not been installed on the test PC. One virtual monitor per host for now. |
| Extend Display, macOS host | **Not available** | Needs a macOS virtual-display helper (CGVirtualDisplay). Mirror works from macOS hosts. |
| Android tablet as Mirror receiver | Built | Via upstream Moonlight for Android + bekdesk's tablet PIN entry. Not tested on a tablet. |
| Android tablet as Extend receiver (Windows host) | Built | Same, with the virtual monitor. Not tested. |
| Android touch controlling the desktop | Upstream | Provided by Moonlight; not tested. |
| bekdesk app for Android / system-wide Android input sharing | **Not available** | Tablets don't appear under Nearby Devices; they use the Tablets & phones section. |
| Simultaneous modes to different devices | Built | Separate session per device; separate Sunshine instance/ports per display target. Changing input targets restarts the Deskflow server (~1 s blip for other input targets). |
| Reconnect after sleep / IP change | Built | Supervisor drops sessions of vanished peers and restores saved modes when they return. |
| Closing the Moonlight window ends that Mirror/Extend session | Built | Mode is set back to Off on the host. |
| Start at login, tray icon, close-to-tray | Built | Tauri autostart plugin + tray. |
| Signed/notarized installers | **No** | Unsigned; OS warnings expected. |

## Known limitations

- Cross-machine drag of a file *with the mouse pointer* across the screen edge is not possible with
  Deskflow; use copy/paste or drop on the device card.
- Interrupted transfers are not resumed; the partial file is removed and the transfer shows *failed*.
- Engines are downloaded on first use, so the first connection needs internet access.
- Sunshine streams are encrypted by Sunshine/Moonlight's own protocol, not the bekdesk TLS channel.
