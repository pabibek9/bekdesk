# Architecture and foundation audit

## Decision

bekdesk is a **thin orchestrator** (Rust + Tauri 2, vanilla HTML/CSS/JS UI) around mature engines
that run as separate processes. It owns everything that makes the product feel like one app —
discovery, identity, pairing, permissions, per-device modes, layout, clipboard and file transfer —
and delegates the latency-critical work to projects with years of hardening.

Why not fork one engine: no single project covers input sharing, real virtual-display extension and
streaming to Android. Forking Deskflow (Qt/C++) would still leave display streaming unsolved;
forking Sunshine would leave input sharing unsolved. Running them as processes keeps each upgradeable
by bumping a hash in `engines.json`, avoids GPL-2.0-only/GPL-3.0 linking conflicts, and keeps the
bekdesk binary small.

Why Tauri: Rust gives a small, memory-safe core with good TLS/mDNS crates; Tauri uses the OS webview
(WebView2 / WKWebView), so the Windows installer is ~2.5 MB with no bundled browser, and the UI is plain CSS.
Flutter or Qt would add a large runtime for no gain here.

## Audit (October 2026)

| Project | What it really is | Verdict |
|---|---|---|
| deskflow/deskflow v1.27.0 | Mature KVM (Synergy lineage), Win/macOS/Linux, text/HTML/image clipboard, TLS | **Used** for input. CLI verified locally: settings-file driven (`deskflow-core server -s file.ini`), screens declared as `[computer_N]` in 1.27. |
| LizardByte/Sunshine 2026.914 | Low-latency GameStream host, HW encode, active | **Used** as display host. Verified locally: per-instance config, `--creds`, 2026 PIN API (`GET/POST /api/pin` with `pairing_id`). |
| moonlight-qt 6.2.0 | Low-latency client, Win/macOS/Linux; Android app separately | **Used** as viewer. Verified locally: `pair <host> --pin`, `list`, `stream`. |
| VirtualDrivers/Virtual-Display-Driver 25.7.23 | IddCx virtual monitor, catalog signed by SignPath Foundation | **Used** for Windows Extend. Package hash and signature checked. |
| akinalpfdn/osplay v0.1.1 | MIT, real Win↔macOS extension over Noise; 11 stars, GUI-only | Not used yet: no automation interface. Best candidate for macOS-hosted Extend. |
| phun333/ShareCursor | Rust KVM with file drag, MIT/Apache | Not used: pre-release, no published releases. |
| peetzweg/opendisplay | macOS virtual display → Apple/Android receivers, GPL-3.0 | Not used: macOS-sender only. |
| localsend/localsend | LAN file transfer app | Not used: file transfer is built into Share Input instead, so no second app is needed. |
| ClassicOldSong/Apollo | Sunshine fork with built-in virtual display | Not used: no release since July 2025; upstream Sunshine + VDD covers it. |

## Components (`src-tauri/src`)

- `identity.rs` — per-device self-signed certificate; SHA-256 of it is the device ID. Pairing code (6 digits) derived from both IDs.
- `discovery.rs` — mDNS/DNS-SD `_bekdesk._tcp`; advertises name, OS, fingerprint, monitor layout. Survives IP changes (re-resolve).
- `net.rs` — mutual TLS 1.3 on TCP 47290. Certificates are pinned by fingerprint, never CA-validated. One request line per connection, then raw bytes. Handles pairing approval (Allow Once / Always Allow / Decline / Block), capability checks, Deskflow tunnel, clipboard, file transfer.
- `modes.rs` — per-device sessions and a supervisor loop: restarts crashed engines, drops sessions whose peer vanished, restores saved modes when trusted devices return (sleep, reboot, new IP).
- `clip.rs` — clipboard watcher; text/images pushed to Share Input peers, copied files transferred and placed on the peer's clipboard.
- `display.rs` — Windows virtual monitor: one-time driver install behind a UAC prompt, then attach/detach per session without elevation.
- `engines.rs` — download, SHA-256 verify, extract, spawn.

## Security model

- Nothing is reachable without the TLS handshake proving possession of a device key.
- Unknown devices can only send a pairing request; the target user must approve it on screen and can compare the 6-digit code shown on both sides.
- Each trusted device has separate switches: control this computer, clipboard, files, screen. They are enforced on the receiving side.
- Deskflow listens on 127.0.0.1 only, with its own TLS off; remote access is solely through the authenticated bekdesk tunnel.
- Sunshine's web UI is restricted to localhost (`origin_web_ui_allowed = pc`) with random per-instance credentials. Its streaming ports are LAN-reachable but require Sunshine's own client pairing, which bekdesk completes only for the device the user chose.
- Received file paths are sanitised (no absolute paths, `..`, drive letters). Files land in `Downloads/bekdesk/<device>-<time>/`.
- The only elevation is the Virtual Display Driver install, shown as a normal UAC prompt.
