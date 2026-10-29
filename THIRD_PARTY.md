# Third-party software

bekdesk itself is licensed under GPL-3.0-or-later (see `LICENSE`).

## Engines (separate programs, downloaded on first use)

bekdesk does not link or bundle these programs. When a feature first needs one, bekdesk downloads
the official release from the project's GitHub Releases page, verifies the SHA-256 pinned in
`engines.json`, and runs it as a separate process. Each keeps its own license; source code is
available at the linked tag.

| Engine | Used for | Version | License | Source |
|---|---|---|---|---|
| Deskflow | Share Input (keyboard/mouse) | 1.27.0 | GPL-2.0 with OpenSSL exception | https://github.com/deskflow/deskflow/tree/v1.27.0 |
| Sunshine | Display host for Extend/Mirror | 2026.914.233613 | GPL-3.0 | https://github.com/LizardByte/Sunshine/tree/v2026.914.233613 |
| Moonlight (moonlight-qt) | Display viewer on Windows/macOS | 6.2.0 | GPL-3.0 | https://github.com/moonlight-stream/moonlight-qt/tree/v6.2.0 |
| Virtual Display Driver | Virtual monitor for Extend on Windows | 25.7.23 | MIT | https://github.com/VirtualDrivers/Virtual-Display-Driver/tree/25.7.23 |

Android receivers are the upstream Moonlight for Android app (GPL-3.0,
https://github.com/moonlight-stream/moonlight-android) or its fork Artemis (GPL-3.0,
https://github.com/ClassicOldSong/moonlight-android), installed by the user from their official sources.

## Rust crates and UI runtime

The bekdesk binary links Rust crates under MIT, Apache-2.0, ISC, BSD and similar permissive
licenses, all compatible with GPL-3.0. Major ones: Tauri (MIT/Apache-2.0), tokio (MIT),
rustls and tokio-rustls (MIT/Apache-2.0/ISC), ring (ISC-style), rcgen (MIT/Apache-2.0),
mdns-sd (MIT/Apache-2.0), clipboard-rs (MIT), reqwest (MIT/Apache-2.0), zip (MIT),
windows-sys (MIT/Apache-2.0). The exact dependency set is pinned in `src-tauri/Cargo.lock`;
each release attaches `THIRD_PARTY_LICENSES.html` generated from it with `cargo-about`.

## Projects evaluated but not used as code

- osplay (MIT) — Windows↔macOS virtual-display extension; young (v0.1.x), GUI-only, no automation interface. Candidate engine for macOS-hosted Extend later.
- ShareCursor (MIT/Apache-2.0) — Rust KVM with file drag; pre-release, no releases published.
- OpenDisplay (GPL-3.0) — macOS-only virtual display sender to Apple/Android receivers.
- LocalSend — file transfer protocol/app; its role is covered by bekdesk's own authenticated transfer inside Share Input.
- Apollo/Artemis (GPL-3.0) — Sunshine fork with built-in virtual display; last release July 2025, so upstream Sunshine + Virtual Display Driver was chosen.
