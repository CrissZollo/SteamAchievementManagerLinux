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

## Install

From a [release](../../releases), either:

```sh
# AppImage: one file, nothing to install
chmod +x sam-*-x86_64.AppImage
./sam-*-x86_64.AppImage

# or the tarball, if you want it on your PATH
tar xzf sam-*-x86_64-linux.tar.gz && cd sam-*-x86_64-linux
./install.sh          # ~/.local/bin, no root needed
```

## Build from source

```sh
cd rust
make            # cargo build --release
make install    # to ~/.local/bin plus a .desktop entry and icons
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

## Releasing

Push a tag and GitHub Actions builds and publishes everything:

```sh
# bump `version` in rust/Cargo.toml first, then:
git tag -a v0.1.0 -m "v0.1.0"
git push origin v0.1.0
```

`.github/workflows/rust-release.yml` builds the tarball, attaches it to a
GitHub release along with a SHA-256 file, and fails early if the tag does not
match the version in `rust/Cargo.toml`.

The release job builds inside an `ubuntu:22.04` container **on purpose**. The
binary links only `libc`, `libm` and `libgcc_s` — X11, Wayland and GL are
`dlopen`ed at run time — so the glibc it is compiled against is the single
thing deciding who can run it. Ubuntu 22.04 ships glibc 2.35, which covers
Ubuntu 22.04+, Debian 12+, Fedora 36+, RHEL 9+ and the rolling distributions.
Building on the bare runner, or on a rolling-release desktop, pins a far newer
glibc and locks most users out.

To build the artifacts locally:

```sh
make dist            # tarball from target/release/sam as-is
make appimage        # single-file AppImage from the same binary
make dist-portable   # tarball, but inside the ubuntu:22.04 container
```

Both print the minimum glibc of what they just built. If that number is higher
than 2.35, the artifact is not fit to publish — use `make dist-portable`, which
needs a running Docker daemon, or let CI do it.

### On AppImage

The AppImage is offered because one downloadable file is convenient, **not**
because it improves portability. AppImage does not bundle glibc — its own
documentation tells you to build on the oldest distribution you want to
support, which is exactly what the release container does. Nor does it bundle
any libraries here, because there are none to bundle: `sam` links only `libc`,
`libm` and `libgcc_s`. X11, Wayland and GL are `dlopen`ed and deliberately left
to the host, since shipping our own `libGL` is a reliable way to break
someone's drivers.

Two details that matter:

- It is built with the **static AppImage runtime**, so it does not require
  `libfuse2`, which Ubuntu 22.04 and later no longer install by default.
- The picker launches each game in its own process. Inside an AppImage it
  execs `$APPIMAGE` rather than `/proc/self/exe`, because the latter points
  into the runtime's FUSE mount, which is torn down when the launching process
  exits and would pull the filesystem out from under a running child.

### Artifacts

| File | Contents |
|------|----------|
| `sam-<version>-x86_64.AppImage` | Single-file bundle, plus `.sha256` |
| `sam-<version>-x86_64-linux.tar.gz` | Binary, `install.sh`, `.desktop`, icons, README, licence, plus `.sha256` |

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
