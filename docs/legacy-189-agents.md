# Native Minecraft 1.8.9 Android: six-agent work plan

> 🤖 Filed by taldoshawn's AI agent (ChatGPT, GPT-6), not by taldoshawn personally.
>
> Read [../AGENTS.md](../AGENTS.md) before contributing. This document is a set of
> tasks and release gates; it does **not** start autonomous agents by itself.

## Target

Port the Java 1.8.9 game rules, protocol 47, assets and worlds into a
**playable native Rust engine** for the Android launcher, without ViaProxy or
bundling Mojang's copyrighted files in the Git repo.

The code in PR #1 currently compiles only network/world primitives. It is not
yet a complete game. Test results for a protocol crate do not replace actual
singleplayer and multiplayer tests on Android.

## Workstreams and ownership

| Issue | Agent scope | Areas |
|---|---|---|
| #2 | Renderer | chunks, 1.8 blocks + metadata, 1.8 resource assets, lighting, entities |
| #3 | Multiplayer | 1.8 login, packet coverage, game-thread event integration, sessions |
| #4 | Singleplayer | terrain, worldgen, Anvil/level.dat, simulation, lighting, save safety |
| #5 | PvP | 20 TPS, collision, sprint reset, sword blocking, velocity, hitboxes |
| #6 | Android | touchscreen, native version launch, ARM64, frame-time/memory |
| #7 | Mods and QA | legacy plugin channels, Forge negotiations, test matrix, release |

Each agent should claim its issue before work, branch from the integration PR,
and open a narrow PR with tests and the GitHub bot attribution in AGENTS.md.
No parallel writes to overlapping files without explicit coordination.

## Shared interfaces

- Protocol 47:
  \`horizon_protocol::legacy_v1_8\`, with \`session::LegacySession\`,
  \`state::LegacyGame\`, \`play\`, \`world\`, \`inventory\`, \`physics\`.
- Native rendering: \`horizon-client-render\`; the existing 26.3
  \`game/remote_chunks.rs\` is *not* compatible with old packed IDs.
- Game loop: \`crates/horizon-client/src/game.rs\`.
- Android entry point: \`crates/horizon-client/src/android_main.rs\`.
- Input: \`crates/horizon-client/src/game/android_input.rs\`.
- Save data: 1.8 Anvil/NBT should not be rewritten with newer save semantics.

## Integration and correctness checkpoints

1. Read/load a 1.8.9 asset pack from the player's legitimate vanilla installation.
2. Render block IDs + metadata from a real 1.8.9 server, with correct faces,
   fluid surfaces, tile entities, biome colors and light.
3. Enter a real protocol-47 server, move with Android touch controls,
   receive block changes, spawn entities, click inventory, and handle disconnect.
4. Create, save, reload and interact with a singleplayer *normal* seeded 1.8.9
   world; compare samples against vanilla and preserve original saves.
5. Test 1.8 PvP with server-authoritative hit registration/knockback and
   no 1.9 cooldown, against an unmodified 1.8.9 server.
6. Verify the selected Forge modded servers **by actual required mods**:
   detecting \`FML|HS\` alone isn't support for Java Forge mods.
7. Build the actual native ARM64 1.8.9 engine APK; install on 4 GB RAM Android,
   profile FPS, frame-times, memory and 30-minute stability.

Only after every applicable checkpoint passes should the launcher advertise a
"Native 1.8.9" playable release. Any earlier APK is a 26.3/experimental build.
