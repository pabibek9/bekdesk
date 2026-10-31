# Sharing one keyboard and mouse with Android phones and tablets

Status: **research only, nothing built or tested on a real device.** Today an Android tablet or phone works with
bekdesk only as a screen (Extend/Duplicate through the Moonlight app), where the PC's own mouse already works.

## What exists

| Option | Root? | Android | What it does | Fit for bekdesk |
|---|---|---|---|---|
| [Deskflow Android](https://github.com/jglanz/deskflow-android) ([F-Droid](https://f-droid.org/en/packages/org.tfv.deskflow/)), MIT, last release 1.0.14 (July 2025) | No | 14+ (`minSdk 34`) | A Deskflow *client*: the PC's mouse crosses onto the phone. Pointer and taps through an **Accessibility service** (on-screen cursor + gestures), typing through its own **keyboard (IME)**, text clipboard. Scrolling by drag doesn't work; arrow keys do. | Works with a normal Deskflow server, but connects straight to it over the network and **can't use client certificates**. |
| [scrcpy](https://github.com/Genymobile/scrcpy) keyboard/mouse only (`--no-video`, UHID/AOA) | No, but needs **Developer options + (wireless) debugging** | 11+ for wireless | Simulates a physical keyboard and mouse on the phone; very good input fidelity. | Wireless debugging has to be paired per device and switches off after a reboot: too fiddly for everyday use. Some modes are USB-only. |
| Barrier / Input Leap / old Synergy Android clients | — | — | No maintained Android client ([Barrier #189](https://github.com/debauchee/barrier/issues/189), [discussion #1305](https://github.com/debauchee/barrier/discussions/1305)). | No. |

## Why not just point Deskflow Android at bekdesk

bekdesk's Deskflow server only listens on `127.0.0.1`; other computers reach it through bekdesk's mutual-TLS
channel, so only paired devices can ever receive keystrokes. Deskflow Android would need the server opened on the
network, and because it can't present a client certificate, the server couldn't tell the real phone from any other
device on the Wi-Fi claiming its screen name. That device would then receive everything typed while the mouse is
on "the phone". bekdesk's rule is that unknown devices never get input, so this isn't shipped.

## Android limits any app has to live with

- **Accessibility service must be switched on by the user** in Settings → Accessibility (it can't be enabled
  for them). On Android 13+ a sideloaded app first needs "Allow restricted settings" in App info.
- **No real mouse pointer** for an app: it draws an overlay cursor (needs "Display over other apps") and
  performs taps/swipes with `dispatchGesture`. Fine for clicking, weaker for drag, hover and fine scrolling.
- **Keys:** an accessibility service can't inject arbitrary key events into other apps; typing goes through the
  app's own keyboard (IME), which the user selects once. Shortcuts map to a subset (Back, Home, app switching).
- **Lock screen:** it can't type the PIN or unlock the device, by design.
- **Background:** it must run as a foreground service with a notification, or Android stops it.
- **Play Store:** Accessibility API use needs a policy declaration; F-Droid/direct download doesn't.

## What a small bekdesk Android app needs

1. **Discovery:** browse `_bekdesk._tcp` with `NsdManager`; advertise itself so PCs list it under Nearby Devices.
2. **Identity and pairing:** a self-signed certificate in the Android Keystore; the same mutual-TLS handshake on
   port 47290 and the same 6-digit code / Always Allow flow as the PCs (`Req::Pair`, `Req::Hello`).
3. **Keyboard and mouse:** request `Req::Tunnel` like a PC client and speak the Deskflow client protocol through
   it, so the PC's server needs no change and nothing is opened to the network. Input injection can reuse Deskflow
   Android's MIT-licensed accessibility + IME code.
4. **Later:** clipboard (`Req::Clip`) and receiving files (`Req::Files`), and showing the PC's screen in-app
   instead of Moonlight.

## Recommendation

Build the bekdesk Android app (option above), starting from Deskflow Android's input code and speaking bekdesk's
own encrypted protocol, so phones and tablets are paired and trusted exactly like computers. Target Android 14+
first (what the reused code supports), then consider lowering it. Until that exists, keep using the tablet as an
Extend/Duplicate screen. Nothing here has been run on a phone or tablet yet; treat every limit above as
to-be-confirmed on real devices.
