# Minecraft Java 1.8.x (protocol 47) on Horizon Android — development status

**The app in this source tree does not yet play on Minecraft 1.8 servers.**
It still runs the Minecraft 26.3 game engine. Nothing in the GitHub Actions
workflow or in this document should be presented as a completed 1.8 APK.

The original Android source package had no upstream server-version adapter.
A simple protocol version edit is **not** a port: 1.8 uses a different login
profile shape and has no configuration state, and PLAY packet IDs, registries,
blocks, entities, player movement, chunks, inventories and combat have changed
substantially since 1.8. The old combat rules require additional gameplay work.

## Implemented in this patch

- A separate, memory-safe `horizon_protocol::legacy_v1_8` module.
- Protocol 47 handshake (Status and Login), login start with the historical
  name-only layout, clientbound login decoder, compression, RSA/AES encrypted
  login using a caller's authentication callback, and success/disconnect.
- 1.8 keep-alive encode/decode primitives.
- Additional 1.8.9 PLAY codecs for JoinGame, player position/teleport confirmation, client settings, 0x21 chunk columns and 0x26 bulk chunks. Block ID/metadata arrays and lighting data are decoded and bounds-checked but **not rendered or translated**.
- A Termux/ViaProxy helper at `packaging/android/legacy-proxy/` that can translate 26.3 client traffic to a 1.8.9 server without changing the Horizon APK. This requires Termux and a separate Java process; it has not yet been tested on the phone.
- Unit tests of protocol 47 wire examples and a locally mocked 1.8 offline
  login, including a keep-alive round trip (no public server accessed).
- `legacy18_probe` diagnostic executable for an offline-mode **test server**:
  it signs in, reports and decodes selected PLAY packets, and replies to keep-alives and position corrections. It is
  **not** a game client and it does **not** render a 1.8 world.
- GitHub Actions configured for the `main` or `android` branch, an APK build
  and separate protocol tests. Signing keys are never committed or uploaded.

## Remaining before a real playable Android 1.8 client

1. Integrate protocol 47 login and session management into Android's game
   state without ever feeding version-47 packets to the version-777 decoder.
2. Add a packet translation layer for 1.8 PLAY clientbound **and** serverbound.
3. Translate pre-flattening numeric block IDs and metadata, chunk sections,
   biomes and lighting to Horizon's modern representations.
4. Translate legacy entity metadata, equipment, inventories, windows,
   movement, teleport confirmations, digging and placing packets.
5. Handle 1.8 PvP mechanics (attack cooldown, reach and hit registration)
   and account for server-specific anticheat expectations.
6. Test offline servers first, then authenticated 1.8 servers using a licensed
   Microsoft Minecraft account, then actual Android phone gameplay.

## How to run the already implemented tests

```
cargo test --locked -p horizon-protocol legacy_v1_8 --lib
cargo check --locked -p horizon-protocol --example legacy18_probe
```

To use the optional diagnostic tool with **your own offline-mode test server**:

```
cargo run --locked -p horizon-protocol --example legacy18_probe -- 127.0.0.1:25565 Steve
```

Do not use a real account's access token with this probe. It has no UI and no
account storage; it intentionally rejects server encryption challenges when
no session-auth callback is supplied.

## Building the Android APK with GitHub Actions

This build produces an **experimental Android 26.3 APK containing the new 1.8
protocol foundation, not a working 1.8 gameplay client**.

1. Fork `Vasysik/horizon-android` with branch `android` (or import the entire
   patched source tree into a repository with default branch `main`).
2. Apply this patch and push the changes to `main` or `android`.
3. Open **Actions → Java 1.8 (protocol 47) codec tests** and check the tests.
4. Open **Actions → Android dev APKs → Run workflow**. Select the branch.
5. Download the ARM64 APK from the successful run's **Artifacts** section.

The APK downloads resources from Mojang on first launch. GitHub runners must
have Android NDK 28.2.13676358, SDK 35, JDK 25, Rust 1.95 and Gradle 8.14;
the workflow installs/configures them. It uses only official resources and
still builds the existing Horizon 26.3 Android client.

### APK signing: important

Without `ANDROID_DEV_KEYSTORE_BASE64`, the build generates an ephemeral signing
key, so subsequent runs cannot update an earlier APK in place. To keep the
same Android app's installed data across CI builds, use a private dedicated
keystore as documented in `docs/android-dev-builds.md`. Never commit a key, a
Microsoft token, raw game JARs or generated Mojang assets to GitHub.

## Code locations

- `crates/horizon-protocol/src/legacy_v1_8.rs` — new wire protocol and tests
- `crates/horizon-protocol/examples/legacy18_probe.rs` — isolated login probe
- `crates/horizon-client/src/net.rs` — existing 26.3-only game session; requires
  future integration (no changes yet to prevent accidental decoding as 26.3)
- `.github/workflows/legacy-18-tests.yml` — new codec test workflow
- `.github/workflows/android-dev.yml` — modified APK build workflow

## Alternative path to play now: local ViaProxy

See `docs/legacy-1.8-play-now.md` and `packaging/android/legacy-proxy/README.md`.
This keeps Horizon 26.3 but routes a 1.8.9 server through an official third-party
protocol translator. **Not** a native 1.8.9 APK; requires a separate Termux
process, additional RAM and licensed account auth for premium servers.

## Incremental Rust work after the initial codec patch

This fork adds packet 47 chat send/receive, UseEntity/attack encoding, player
position encoding, entity removal decoding and relative-entity movement
decoding, with local round-trip test vectors. These functions are intentionally
not connected to the 26.3 game session yet. A 26.3 packet cannot be sent to
a 1.8 server just by changing its packet ID.

The Android Rust launcher now exposes an **external ViaProxy mode** via its
version selector; that is an optional interoperability bridge, not a replacement
for the incomplete native protocol implementation.
