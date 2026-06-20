# wreckfest-teleport

Automatic telemetry for **Wreckfest 1** (2018), which ships **no** telemetry API
— no shared memory, no UDP. This tool reads the running game's memory, finds the
player car's transform, and turns it into real motion telemetry for SimHub
(bass shakers, motion rigs, dashes).

It is **library-first** so it can fold into the `sim-teleport` monorepo later as
`crates/wreckfest-teleport` with no restructuring.

## What it can and cannot do (read this first)

Wreckfest 1 only exposes the car's **transform matrix** (position + orientation)
in a readable form. That is confirmed by the SpaceMonkey project, which did the
original reverse-engineering and could not find real engine/drivetrain data
either (it fakes RPM from the gamepad trigger and hardcodes gear). This tool does
**not** fabricate anything.

**Real data we provide** (read from the matrix, or derived from it over time with
real physics):

- World position (x, y, z)
- Orientation: pitch, yaw, roll
- Velocity (world and car-local: surge / sway / heave) and scalar speed
- G-forces: lateral, longitudinal, vertical (great for bass shakers / motion)
- Angular rates: pitch / yaw / roll rate
- Impact magnitude (a derived crash signal from sudden g-force changes)

**Not available — and therefore intentionally absent** (Wreckfest 1 does not
expose these in memory): engine RPM, gear, throttle / brake / clutch, suspension,
tyre temps/pressures, fuel, flags, lap/sector data.

> Practical consequence: motion and crash haptics work great. Fanatec RPM shift
> lights and gear display cannot work for Wreckfest 1, because the game does not
> provide real RPM or gear to read.

## Build (Windows)

Requires Rust (stable, 1.75+). Wreckfest 1 is 64-bit, so build 64-bit:

```
cargo build --release
```

The binary is `target/release/wreckfest-teleport.exe`. Tagging `vX.Y.Z` triggers
CI (`.github/workflows/release.yml`, `windows-latest`) which publishes the
`.exe` to GitHub Releases.

## Run

Two modes.

**Console (start here — proves we're reading real data):**

```
wreckfest-teleport
wreckfest-teleport console --rate 100
```

Launch Wreckfest, start a race, and watch live speed / position / g-force / yaw
rate / impact update as you drive. This is the verification step.

**UDP (stream to the SimHub PC):**

```
wreckfest-teleport udp --target 192.168.50.2:22123
wreckfest-teleport udp --target 192.168.50.2:22123 --format native
```

Options: `--target IP:PORT`, `--rate HZ` (default 100), `--slot N` (default 0;
single-player is always 0), `--format native|simhub`.

It auto-waits for the game, scans automatically, validates the lock (re-scans if
the car despawns), and returns to idle when the game closes. No per-race setup.

## SimHub wiring (the one step that must happen in SimHub)

SimHub's External Sim packet includes a header with game/telemetry **signatures
that SimHub computes from your `.simdef`** — those exact bytes can only come from
SimHub itself, so they are deliberately not guessed here.

1. In SimHub, open the **External Sim** editor and create a definition using the
   fields documented in [`wreckfest.simdef`](./wreckfest.simdef) (real fields only).
2. Click **Copy demo code (C# / C++)**. That gives the exact packet struct,
   constants, and header.
3. Paste that struct back so it can be wired into `emitter.rs::pack_simhub()`.
   Until then, `--format simhub` sends the native packet as a placeholder and
   will not validate against SimHub.
4. Install the definition under `%localappdata%/SimHub/ExternalSims/` and select
   the sim in SimHub (cross-PC setups won't auto-detect the process).

The `native` format (documented in `emitter.rs`) is fully defined now and can be
verified with any loopback UDP receiver.

## How it finds the data (grounded, not guessed)

Ported faithfully from SpaceMonkey (MIT — see [`NOTICE`](./NOTICE)):

1. Scan process memory for the ASCII string `carRootNode00` (`00` = player slot;
   single-player is always 00).
2. Step **136 bytes** back (`((4*4*4)*2)+8`) to the start of a 4x4 float
   transform matrix; read 64 bytes.
3. Position = translation row; orientation = rotation basis. Validate by checking
   the basis vectors are unit-length (rejects garbage / wrong lock).
4. Derive velocity, g-force, angular rates and impact from the transform sampled
   over time.

Read-only access only (`PROCESS_VM_READ | PROCESS_QUERY_INFORMATION`); the game
is never written to or injected. Single-player / offline use.

## Status

Core reader + derivation complete and unit-tested. Pending on-rig validation:
confirm the lock and live values while driving, then wire the SimHub packet from
the generated `.simdef` demo code.
