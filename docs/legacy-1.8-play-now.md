# Play on 1.8.9 servers with Horizon Android via a local translation proxy

The native gameplay translation for protocol 47 is **not complete**. The
`horizon_protocol::legacy_v1_8` and `legacy_v1_8::play` modules decode selected
1.8 login and PLAY wire data but are not integrated into the Android renderer.

To actually try connecting the existing 26.3 Horizon Android client to a 1.8.9
server now, use the optional standalone ViaProxy helper at
`packaging/android/legacy-proxy/start-termux.sh`, documented in the adjacent
README. It runs in Termux; it is not bundled into the APK, which remains a
native Rust Android app. No unsupported Java JAR is embedded in Horizon.

Official ViaProxy compatibility: https://github.com/ViaVersion/ViaProxy
Termux OpenJDK 25 recipe: https://github.com/termux/termux-packages/tree/master/packages/openjdk-25

This path still needs real-phone server testing, especially for authenticated
servers and older anti-cheat. A standalone APK that plays 1.8.9 without a
proxy is **not yet built**.

## New Rust launcher integration (Pojav-inspired build)

The native Android launcher now has a version selector with an explicit
`Minecraft 1.8.9 server - ViaProxy` option. In this mode Play routes the
existing 26.3 game through `127.0.0.1:25568` and refuses to launch when a
local TCP listener is unavailable. This does NOT replace the engine with 1.8.9
or eliminate the need for the external proxy. See
`docs/pojav-inspired-rust-189.md` for exact steps.
