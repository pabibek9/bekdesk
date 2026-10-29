# bekdesk

Use your Windows PCs, Macs and Android tablet together over your local network. Open bekdesk, your
other computers show up under **Nearby Devices**, click one and choose:

- **Share Input**: one keyboard and mouse across computers, with clipboard sync and file copy/paste.
- **Extend**: the other computer's screen becomes an extra monitor for this one (Windows hosts for now).
- **Mirror**: show this computer's main screen on the other device.

Each device has its own mode, and the modes run at the same time. No accounts, no cloud, no subscription.

> **Status: 0.1.0 preview.** Discovery, pairing, the encrypted channel and file transfer are tested.
> Input sharing and display streaming drive mature upstream engines but have **not yet been tested
> across two physical machines**. See [docs/STATUS.md](docs/STATUS.md) before relying on it.

## Install

Download from [Releases](https://github.com/pabibek9/bekdesk/releases):

| Platform | File |
|---|---|
| Windows 10/11 x64 | `bekdesk-Windows-x64-Setup.exe` |
| macOS 11+ (Apple Silicon and Intel) | `bekdesk-macOS-universal.dmg` |
| Android tablet | Install **Moonlight** (Play Store / F-Droid) — see below |

Installers are **not code-signed** yet: Windows SmartScreen and macOS Gatekeeper will warn the first
time. Full guide: [docs/INSTALL.md](docs/INSTALL.md).

## How it works

bekdesk is a small Rust/Tauri app (Windows installer ~2.5 MB, macOS universal dmg ~6 MB) that handles discovery, pairing, security, layout and file
transfer itself, and drives proven open-source engines for the hard real-time parts:

| Job | Engine |
|---|---|
| Keyboard/mouse sharing | [Deskflow](https://github.com/deskflow/deskflow) |
| Screen streaming host | [Sunshine](https://github.com/LizardByte/Sunshine) |
| Screen streaming viewer | [Moonlight](https://github.com/moonlight-stream/moonlight-qt) |
| Virtual monitor (Windows) | [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) |

Engines download from their official releases the first time a feature needs them, verified against
SHA-256 hashes pinned in [`engines.json`](engines.json). Details and the project audit:
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Build

CI builds installers on GitHub Actions ([`.github/workflows/build.yml`](.github/workflows/build.yml)).
Locally: Rust stable + Node 20, then

```bash
cd src-tauri && cargo test --lib --test e2e && cd .. && npx @tauri-apps/cli@2 build
```

## License

GPL-3.0-or-later. Third-party notices: [THIRD_PARTY.md](THIRD_PARTY.md).
