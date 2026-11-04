# Pojav-inspired Horizon Android launcher, written in Rust

## What this change ACTUALLY does

- The **existing native Horizon 26.3 Rust game** is retained. No Java game engine,
  PojavLauncher Java classes, layout XML or Pojav resource files are shipped.
- The Rust launcher has a Pojav-like layout: account/profile row, settings,
  navigation drawer, prominent version selector, a broad green Play button,
  and shortcuts to files, controls, logs and mods where there is room.
- The in-game touch overlay adds a centered Tools drawer handle on wide
  screens, using the existing Rust `TouchUi::Action::More` control; the
  original side handle remains available for accessibility.
- Choosing `Minecraft 1.8.9 server - ViaProxy` saves an explicit launch mode in
  `android-launcher.json`. **The label deliberately says server and ViaProxy**:
  this is NOT the Minecraft 1.8.9 executable.
- When you press Play in that mode, the app checks `127.0.0.1:25568` locally.
  If it is unavailable, it displays an error instead of launching a broken
  connection. When available, the native 26.3 client starts with
  `--server 127.0.0.1:25568`. The local proxy must be configured to connect
  to the intended Minecraft 1.8.9 server.
- `Native26_3` keeps the server address specified in Settings unchanged.
  Both modes are persisted with a backward-compatible serde default.

## What this change does NOT do

This is **not a complete native 1.8.9 game port**. The 1.8 protocol 47 codec
has additional Rust wire packets for chat, player position, entity movement,
entity removal and attack packets, but **it is not connected to Horizon's
renderer/simulation/inventory or native network play session**. The external
ViaProxy translator is required to connect to 1.8 servers. Old 1.8 PvP
mechanics and mods are NOT guaranteed.

## Try the bridge on a phone

1. Install Termux, and grant its storage permission.
2. Extract `packaging/android/legacy-proxy/` to your accessible Downloads
   folder. Run the `start-termux.sh` file with the **actual** server hostname:

   ```sh
   termux-setup-storage
   bash ~/storage/downloads/legacy-proxy/start-termux.sh example.org:25565
   ```

   The path above is just an example: adjust it to the location where you
   extracted the folder. Do not enter `example.org` literally.
3. Configure ViaProxy authentication if the target is online-mode/premium.
   In-game sign in with an authorized Minecraft account.
4. Keep Termux running. In Horizon Android choose the `1.8.9 server - ViaProxy`
   option on the Play page and tap Play. It uses the fixed, private loopback
   port `127.0.0.1:25568`. The Termux helper also binds only to loopback.
5. If anything fails, open Logs and record the disconnect text. Confirm your
   actual server accepts the ViaProxy version. This flow has **not yet been
   verified on a physical Android device**.

> The optional helper requires Java in a *separate Termux app* to run ViaProxy.
> The Horizon renderer, UI, launcher and native client remain in Rust.

## Testing and Android build

- `python3 -m unittest discover -s tests -p 'test_android*.py'`
- `cargo test --locked -p horizon-protocol legacy_v1_8 --lib`
- `cargo check --locked -p horizon-protocol --example legacy18_probe`
- The full Android build uses `.github/workflows/android-dev.yml` after you
  commit this source tree to an `android` or `main` branch in GitHub. Select
  Actions > Android dev APKs > Run workflow, then download the ARM64 artifact.

The first command can run locally without Rust. The two Cargo commands and
Android build must run in an environment with Rust/NDK/JDK/Gradle; they were
**not** executed in the file preparation environment.

## Source reference / licensing

UI hierarchy was studied from the user-supplied PojavLauncher 3 source tree,
particularly `fragment_launcher.xml` and `activity_pojav_launcher.xml`. The
resulting widgets were independently implemented using Horizon's existing Rust
`Ui`/`HudBatch` APIs and assets. No PojavLauncher source code, images or fonts
were included in the Horizon package. Keep Horizon's existing third-party
licenses and notices for all dependencies.
