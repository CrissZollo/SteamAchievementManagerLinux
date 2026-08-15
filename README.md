# Steam Achievement Manager — Linux

[![Rust CI](https://github.com/CrissZollo/SteamAchievementManagerLinux/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/CrissZollo/SteamAchievementManagerLinux/actions/workflows/rust-ci.yml)

Steam Achievement Manager (SAM) is a lightweight tool for managing achievements
and statistics in Steam. This is a fork of
[gibbed/SteamAchievementManager](https://github.com/gibbed/SteamAchievementManager)
that adds a **native Linux port written in Rust**, so it runs as an ordinary
Linux process instead of under Wine.

Steam must be installed natively, running, and signed in.

## Why this fork exists

Running the Windows build under Wine does not work. Wine's `steamclient.dll`
looks for a Steam client *inside the prefix*, so a Steam installed natively on
the host is invisible to it — and installing a second Steam inside the prefix
just to manage achievements is not a reasonable answer.

The Linux port talks to `steamclient.so` directly, so it sees the Steam client
you are already running.

## Install

Grab a [release](https://github.com/CrissZollo/SteamAchievementManagerLinux/releases/latest):

```sh
# AppImage: one file, nothing to install
chmod +x sam-*-x86_64.AppImage
./sam-*-x86_64.AppImage

# or the tarball, if you want it on your PATH
tar xzf sam-*-x86_64-linux.tar.gz && cd sam-*-x86_64-linux
./install.sh          # ~/.local/bin, no root needed
```

Builds target glibc 2.35, so they run on Ubuntu 22.04+, Debian 12+, Fedora 36+,
RHEL 9+ and the rolling distributions. x86-64 only, since that is what Steam
ships `steamclient.so` for.

There are no GTK or Qt dependencies — the GUI is pure Rust and the binary links
only `libc`, `libm` and `libgcc_s`.

## Usage

```sh
sam                 # browse owned games that have achievements
sam --app 620       # open the editor for one game directly
```

Click a game in the picker to open its editor, tick the achievements you want,
then press **Store**.

Each game opens in its own process, which is deliberate: `steamclient.so` reads
`SteamAppId` when it initialises, and a process that has bound to one app can
never rebind to another. The Windows original has the same constraint, which is
why it ships a separate `SAM.Game.exe`.

## Differences from the Windows version

- **Game discovery works offline.** Steam caches an achievement schema for each
  app under `appcache/stats/`, so your library lists instantly with no network
  round trip, and every entry is known to have something to edit. "Find more…"
  falls back to the remote master list to catch games you own but have never
  launched.
- **Unlock times are shown in UTC**, to avoid a timezone dependency.
- **Statistics editing is off by default**, behind an "Allow editing" toggle.
  Stats often drive achievement progress and server-side checks, so they are
  riskier to touch than achievements.

## Repository layout

| Path | Contents |
|------|----------|
| `rust/` | The native Linux port. Start at [`rust/README.md`](rust/README.md). |
| `SAM.API/`, `SAM.Game/`, `SAM.Picker/` | The original C#, unmodified. Windows-only, and the reference specification for behaviour. |

## Building

```sh
cd rust
make            # release build
make test       # test suite, no Steam required
make install    # ~/.local/bin plus a .desktop entry and icons
```

Requires Rust 1.85 or newer. See [`rust/README.md`](rust/README.md) for the
architecture, the release process, and a write-up of the ABI differences
between Windows and Linux that the port had to solve.

### Verifying after a Steam update

Steam updates can move things around under the hood. `make probe` checks this
machine's `steamclient.so` against what the port expects — whether the vtable
slots resolve correctly, whether the achievement count agrees with the cached
schema, and which way round the `GetStat` overload pair is. It is strictly
read-only and never writes a stat or unlocks an achievement.

## Safety

Achievements and statistics that a game marks as protected are read-only in the
UI, because Steam will not accept changes to them. "Reset all stats" asks for
confirmation and says plainly that it cannot be undone.

Editing achievements is against Steam's terms for some games and can be
detected. VAC does not ban for it, but individual developers can and do act on
it. Use this on your own single-player games.

## Credits

SAM was written by [Rick "gibbed" Gibbed](https://github.com/gibbed). The
closed-source version was originally released in 2008, last saw a major release
in 2011, and was last updated in 2013; the source was opened later, with
general code maintenance, replacement icons, and a version bump to 7.0.x.x.

This fork adds the Rust/Linux port in `rust/`. The C# tree is unmodified — for
Windows, use [upstream's releases](https://github.com/gibbed/SteamAchievementManager/releases/latest).

Most (if not all) icons are from the
[Fugue Icons](https://p.yusukekamiyamane.com/) set.

Released under the zlib licence; see [LICENSE.txt](LICENSE.txt).
