# Horizon Android 26.3 to Minecraft Java 1.8.9, via local ViaProxy

**This is not a native 1.8.9 port or a standalone APK.** It is a pragmatic,
third-party route to *try* multiplayer on a 1.8.9 server from the existing
Horizon Android 26.3 app, without giving 1.8 packets to Horizon's new-format
decoder.

ViaProxy lists client versions 1.7.2–26.3 and server versions 1.0–26.3 as
supported in its public project README, including their cross-product. It is
third-party GPL-3.0 software, downloaded separately and kept outside Horizon.
Source: https://github.com/ViaVersion/ViaProxy

## Phone-only setup

1. Download/install the official **Termux** app from F-Droid or its releases
   on GitHub. The old Google Play version is not recommended.
2. Download this helper ZIP to Android **Downloads** and extract it there.
3. Open Termux and run:

   ```bash
   bash ~/storage/downloads/horizon-189/start-termux.sh mc.example.org:25565
   ```

   If `~/storage/downloads` is not available, first run `termux-setup-storage`
   and grant access. You can also paste or copy this `.sh` into your Termux
   home without storage permission.
4. On first run the script installs Termux OpenJDK 25, curl and jq, then
   downloads the latest **official** ViaProxy JAR from GitHub Releases, asks
   ViaProxy to generate its own config, sets target `1.8.x` and binds it to
   `127.0.0.1:25568`. Nothing is downloaded from unofficial mirrors.
5. Keep Termux open, and in Horizon Android choose **Multiplayer → Direct
   Connect → `127.0.0.1:25568`**. The proxy translates 26.3 ↔ 1.8.9.

Stop the proxy with **Ctrl+C**. Re-run with another target to switch servers.
Termux can be killed by Android battery management, so consider exempting
Termux from battery optimization if it stops during play.

## Online-mode/paid account servers

This default configuration is intended for **offline-mode test servers**.
Premium 1.8 servers often require ViaProxy to authenticate independently
using the Minecraft account you own. Read ViaProxy's documented account
commands and `auth-method: account` settings before attempting online mode.
Never send your password, access tokens or `saves.json` to anyone or commit
these to GitHub. Configure accounts locally through the officially supported
ViaProxy flow; do not put credentials in this ZIP or scripts.

## Limitations

- The script and ViaProxy have not been run on the user's actual Android device.
  Networking, account login, anticheat and mobile performance still need a
  real-device test. This helper was checked for shell syntax only.
- A current Java launcher and Java 26.3-capable Horizon build are required.
  This does **not** switch Horizon's client physics/animation rules to 1.8.9;
  server anticheat may reject some gameplay and PvP might feel modern.
- ViaProxy and OpenJDK will consume extra RAM and disk space. On a phone with
  4 GB RAM, running this alongside Horizon can cause Android to kill one app.
- The helper intentionally listens only on `127.0.0.1`, not on public Wi-Fi.
- A standalone 1.8.9 *Rust* gameplay client will require the native packet,
  world and entity translation described in `docs/legacy-1.8-port.md`.
