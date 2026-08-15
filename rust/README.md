# Steam Achievement Manager — native Linux port

A Rust rewrite of SAM that runs as an ordinary native Linux process and talks
to the native Steam client directly.

## Why this exists

Running the Windows build under Wine does not work, because Wine's
`steamclient.dll` looks for a Steam client *inside the prefix*. A Steam client
installed natively on the host is invisible to it. Rather than install a second
Steam inside the prefix, this port speaks to `steamclient.so` directly, so it
sees the Steam you are already running.

## Requirements

- Rust 1.85 or newer
- A native Steam installation, running and signed in
- x86-64 (Steam ships `linux64/steamclient.so`)

Nothing else. The GUI is pure Rust, so there are no GTK or Qt build
dependencies, and the result is a single self-contained binary.

Flatpak Steam is detected, but the sandbox may prevent a host-installed `sam`
from reaching the client. A native Steam install is the supported setup.

## Build and install

```sh
cd rust
make            # cargo build --release
make install    # to ~/.local/bin plus a .desktop entry
```

Or straight from cargo:

```sh
cargo run --release -p sam-app
```

## Usage

```sh
sam                 # browse owned games that have achievements
sam --app 620       # open the editor for one app directly
sam --app 620 --screenshot shot.png   # render, save a PNG, exit
```

Click a game in the picker to open its editor.

### One process per game

`steamclient.so` reads `SteamAppId` when it initialises. Setting it later has no
effect, and once a process has bound to an app it cannot rebind — Steam keeps
reporting the original ID. Editing a second game therefore needs a second
process, so the picker spawns `sam --app <id>` rather than switching views in
place. The Windows original has the same constraint, which is why it ships a
separate `SAM.Game.exe`.

## Verifying your setup

Steam updates can move things around. Before trusting a write, you can check
this machine against what the interop layer expects:

```sh
make probe
```

This is strictly read-only — it never writes a stat, unlocks an achievement, or
calls `StoreStats`. It reports the resolved Steam paths, whether the vtable
slots resolve into `steamclient.so`, whether `GetNumAchievements` agrees with
the cached schema, and which way round the `GetStat` overload pair is.

## Layout

| Crate       | Role                                                            |
|-------------|-----------------------------------------------------------------|
| `sam-vdf`   | Steam's binary KeyValues parser (port of `KeyValue.cs`)          |
| `sam-steam` | Bindings to the private `steamclient.so` interfaces (`SAM.API`)  |
| `sam-app`   | The `sam` binary: picker and editor (`SAM.Picker` + `SAM.Game`)  |

## How the interop works

Steam's private client API hands out pointers to C++ objects with no C entry
points, so every call goes through the object's vtable by index. Slot indices
live in `sam-steam/src/interfaces.rs`.

Four things differ from the Windows original, each of which would silently
corrupt data rather than crash:

**Calling convention.** Windows x86 uses `thiscall`, passing `this` in ECX.
SysV x86-64 has no equivalent: `this` is just the first argument, in RDI. Every
signature takes an explicit leading `this`. (The C# has a latent bug here —
`NativeGetISteamApps` omits `self` entirely, which only works because
`thiscall` keeps the receiver out of the stack arguments.)

**Overload ordering.** MSVC emits overloads within one overload group in
*reverse* declaration order; the Itanium ABI uses declaration order.
`ISteamUserStats` overloads `GetStat`, `SetStat` and `GetUserStat` on
`int32` versus `float`, so those pairs are swapped relative to the C#
definitions. This is not hardcoded: `resolve_overload_order` determines it at
run time by calling both slots of the group with a stat whose type the schema
already tells us, and seeing which one Steam accepts. Reads only.

**Callback packing.** The Steamworks SDK sets `VALVE_CALLBACK_PACK_SMALL`
(`#pragma pack(4)`) on Linux and macOS but pack 8 on Windows, deliberately, so
64-bit callback structs keep the 32-bit layout. `UserStatsReceived_t` is
therefore `{u64 @0, i32 @8, u64 @12}`, 20 bytes, not the naturally aligned 24.

**Buffer-filling getters.** `ISteamApps001::GetAppData` does not return a
`const char*` like its neighbours. It is
`int GetAppData(AppId_t, const char *key, char *value, int len)`, filling a
caller-supplied buffer and returning a length. Treating that `int` as a pointer
dereferences a small integer.

Before any of it is called, `Interface::verify_slots` uses `dladdr` to prove
each slot resolves to a symbol inside `steamclient.so`. A Steam update that
reshuffles an interface produces a clear error instead of a wild call.

## Differences from the Windows version

- **Game discovery is local first.** Steam caches an achievement schema per app
  under `appcache/stats/`, so the library can be listed instantly and offline,
  and every entry is known to have something to edit. "Find more…" falls back
  to the remote master list to catch games you own but have never launched.
- **Unlock times are shown in UTC**, since resolving the local timezone would
  mean another dependency.
- **Statistics editing is off by default** behind an "Allow editing" toggle.
  Stats often drive achievement progress and server-side checks.

## Safety

Protected achievements and stats — those whose schema permission bits are set —
are read-only in the UI, as in the original. "Reset all stats" asks for
confirmation and states plainly that it cannot be undone.

Editing achievements is against Steam's terms for some games and can be
detected. VAC does not ban for this, but individual developers can and do act
on it. Use it on your own single-player games.
