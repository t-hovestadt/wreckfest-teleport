# Wreckfest Teleport

Automatic telemetry for **Wreckfest 1** (2018), which ships **no** telemetry —
no shared memory, no UDP, nothing. Wreckfest Teleport reads the running game's
memory, finds your car, and streams real motion telemetry to SimHub using the
Codemasters / DiRT Rally 2.0 UDP format — so your **bass shakers, wind, and
motion effects work** in a game that otherwise gives you nothing to work with.
One small Windows executable, no installer, no dependencies.

```
┌──────────────────────────────┐     UDP — DiRT Rally 2.0 format     ┌────────────────────────┐
│          Gaming PC           │  ────────────────────────────────►  │       SimHub PC        │
│                              │             port 20777              │                        │
│  Wreckfest 1                 │                                     │  SimHub                │
│    └─ process memory         │                                     │    └─ DiRT Rally 2.0   │
│         └─ wreckfest-teleport│                                     │         plugin         │
└──────────────────────────────┘                                     └────────────────────────┘
```

The reader runs on the PC playing Wreckfest (it reads the game's memory there) and
sends UDP to wherever SimHub is — a second PC, or the same one for a local setup.

**Companion projects:**
- [sim-teleport](https://github.com/t-hovestadt/sim-teleport) — unified binary containing iRacing Teleport, AC Teleport, and Sim Relay; automatic game detection, no manual switching

---

## What you get (and what you don't) — read this first

Wreckfest 1 only exposes the car's **transform matrix** (position + orientation)
in readable memory. The SpaceMonkey project did the original reverse-engineering
and likewise found no real engine/drivetrain data — it fakes RPM from the gamepad
trigger and hardcodes gear. **Wreckfest Teleport never fabricates anything.**

**Real data** (read from the matrix, or derived from it over time with real physics):

- World position (x, y, z)
- Orientation: pitch, yaw, roll
- Velocity — world and car-local (surge / sway / heave) — and scalar speed
- G-forces: lateral, longitudinal, vertical (ideal for bass shakers / motion)
- Angular rates: pitch / yaw / roll
- Impact magnitude (a crash signal derived from sudden g-force changes)

**Not available, and therefore intentionally absent** (Wreckfest 1 does not expose
these): engine RPM, gear, throttle / brake / clutch, suspension, tyre temps and
pressures, fuel, flags, lap / sector data.

> **Practical consequence:** motion, wind, and crash/impact haptics work great.
> Anything keyed to engine RPM, gear, or wheel rotation — Fanatec shift lights, a
> gear display, SimHub's wheel-slip effect — cannot work, because the game does
> not provide that data to read. (Wheel-slip in particular should be turned off:
> with no wheel-speed data it reads as permanent slip.)

---

## Download

Pre-built Windows x64 binaries are on the [Releases](../../releases/latest) page.
Grab the `.zip` (everything) or the individual files:

| File | What it's for |
|------|---------------|
| `wreckfest-teleport.exe` | the program |
| `1-verify-console.bat` | sanity check — prints live telemetry to a console window |
| `2-stream-to-simhub.bat` | stream to your SimHub PC (single-player, headless) |
| `3-stream-simhub-local.bat` | stream to SimHub on the **same** PC (single-PC setup) |
| `4-pick.bat` | **multiplayer** — opens a window to pick your car, then streams |

Reading game memory needs Administrator rights; the `.bat` launchers self-elevate
(you'll see a UAC prompt). Edit the IP near the top of a launcher if your SimHub
PC isn't `192.168.50.2`.

---

## Quick start

1. **SimHub** (on whichever PC runs it): enable the **Dirt Rally 2.0** game. It
   listens for telemetry on UDP **20777**. If SimHub is on a different PC from
   Wreckfest, allow inbound UDP 20777 through that PC's firewall.
2. On the PC running Wreckfest, run **`1-verify-console.bat`** first and drive a
   bit — confirm `speed` / `position` / `g-force` update. That proves it's reading
   the game.
3. Run **`2-stream-to-simhub.bat`**. SimHub shows Dirt Rally 2.0 as running, and
   your motion / wind / bass-shaker effects respond as you drive.

You don't edit any game config — Wreckfest doesn't have one. Wreckfest Teleport
*is* the telemetry source.

> SimHub will label the game "DiRT Rally 2.0". That's just the telemetry format it
> understands — it's reading Wreckfest. Cross-PC UDP setups won't auto-detect the
> game, so select it once in SimHub.

---

## Multiplayer

In single-player your car is always the same internal slot, so there's nothing to
do — it's selected automatically. **Online, the game numbers cars by lobby join
order**, which changes every session, so the tool can't know which car is yours up
front.

Run **`4-pick.bat`**: a small window lists every car in the session with a **live
speed**. Brake for a moment and click the car whose speed drops to zero — that's
you. It streams that car immediately; click a different one to switch, or hit
**Rescan** when you join a new lobby. (Single-player auto-selects car 00, so you
can ignore the window there.)

---

## Modes and options

```
wreckfest-teleport [MODE] [OPTIONS]
```

| Mode | Description |
|------|-------------|
| `console` (default) | Print telemetry to the terminal — the verification step |
| `udp` | Stream to SimHub over UDP (headless) |
| `pick` | Same UDP stream **plus** the car-picker window (multiplayer) |

| Flag | Default | Description |
|------|---------|-------------|
| `--target <IP:PORT>` | `127.0.0.1:20777` | UDP destination (udp / pick modes) |
| `--rate <HZ>` | `100` | Poll rate; 60+ recommended for clean derived acceleration |
| `--slot <N>` | `0` | Car slot / initial pick — single-player is always 0 |
| `--format <FMT>` | `simhub` | `native` or `simhub` (simhub = Codemasters extradata=3) |
| `-h`, `--help` | | Show help |
| `-V`, `--version` | | Show version |

It auto-waits for the game, scans automatically, validates the lock (and re-scans
if the car despawns), and returns to idle when the game closes — no per-race setup.

---

## How it works (grounded, not guessed)

The memory-location method and motion math are ported faithfully from SpaceMonkey
(MIT — see [`NOTICE`](./NOTICE)):

1. Scan process memory for the ASCII string `carRootNodeNN`, where `NN` is the
   car's slot (single-player is always `00`).
2. Step **136 bytes** back (`((4*4*4)*2)+8`) to the start of a row-major 4×4 float
   transform matrix; read 64 bytes.
3. Position is the translation row; orientation is the rotation basis. The lock is
   validated by checking the basis vectors are unit-length, which rejects garbage
   or a stale/dead copy of the node.
4. Velocity, g-force, angular rates, and impact are derived from the transform
   sampled over time (with smoothing and clamping so scheduler hitches don't spike
   the output).
5. The result is packed as a Codemasters **extradata=3** packet (264 bytes, 66
   little-endian floats): position, world velocity, the orientation direction
   vectors, speed, and lateral/longitudinal g — every engine/wheel field left
   zero. SimHub's DiRT Rally 2.0 plugin reads it natively on UDP 20777.

Access is **read-only** (`PROCESS_VM_READ | PROCESS_QUERY_INFORMATION`); the game
is never written to or injected into.

---

## Build

Requires Rust (stable, 1.75+). Wreckfest 1 is 64-bit, so build 64-bit:

```
cargo build --release
```

The binary is `target/release/wreckfest-teleport.exe`. Tagging `vX.Y.Z` triggers
CI (`.github/workflows/release.yml`, `windows-latest`), which publishes the `.exe`
and launchers to GitHub Releases.

---

## Credits

The method for locating Wreckfest's telemetry in memory, and the math for deriving
motion from the car transform, were learned from and reimplemented after the
[SpaceMonkey](https://github.com/PHARTGAMES/SpaceMonkey) project (MIT). No
SpaceMonkey source is included verbatim; see [`NOTICE`](./NOTICE) for details.
SpaceMonkey is unaffiliated with this project.

## License

MIT.
