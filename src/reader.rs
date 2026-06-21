//! The headless reader loop.
//!
//! This is the whole point of wreckfest-teleport vs SpaceMonkey: no GUI, no
//! manual "Initialize" per event. It waits for the game, scans automatically,
//! validates the lock, streams telemetry, and silently re-scans or returns to
//! idle when the game closes or the lock goes stale.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::math::Transform;
use crate::process::{find_process_pid, ProcessHandle};
use crate::scan::{locate_matrix_candidates, ScanResult};
use crate::signatures::MATRIX_SIZE_BYTES;
use crate::telemetry::{Deriver, Telemetry};

/// Reader configuration.
#[derive(Clone, Copy, Debug)]
pub struct ReaderConfig {
    /// Player car slot. Single-player is always 0.
    pub slot: u8,
    /// Poll rate in Hz. SimHub wants >=60; 100 gives clean derived accel.
    pub poll_hz: u32,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            slot: crate::signatures::DEFAULT_PLAYER_SLOT,
            poll_hz: 100,
        }
    }
}

/// Lifecycle status, surfaced to the sink so callers can log/react.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    WaitingForGame,
    Scanning,
    Locked { matrix_address: usize, slot: u8 },
    LockLost,
    GameClosed,
}

/// Where telemetry and status updates go. Implement this for console output, a
/// UDP emitter, or any other consumer.
pub trait TelemetrySink {
    fn on_status(&mut self, status: Status);
    fn on_frame(&mut self, frame: &Telemetry);
}

/// Drop the lock and re-scan after this many consecutive invalid reads.
const MAX_INVALID_FRAMES: u32 = 30;
/// How long a locked transform may stay byte-identical before we treat it as a
/// stale/despawned lock and re-scan. A live car (even idling) jitters; a dead
/// leftover copy of the node does not.
const STALE_LOCK_SECS: f32 = 1.5;
/// Gap between the two reads used to probe a candidate's liveness.
const LIVENESS_PROBE: Duration = Duration::from_millis(80);

fn parse_matrix(bytes: &[u8; MATRIX_SIZE_BYTES]) -> Transform {
    let mut m = [0f32; 16];
    for (i, slot) in m.iter_mut().enumerate() {
        let o = i * 4;
        *slot = f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    }
    Transform::from_floats(m)
}

/// Choose a validated, *live* lock among all candidate addresses.
///
/// Wreckfest leaves stale copies of `carRootNode00` in freed memory after a race
/// change, so the first match is not always the live car -- a dead copy's
/// transform never updates. We keep every basis-valid candidate, then read each
/// twice a short gap apart and prefer one whose bytes changed (the live car
/// jitters even at rest; a dead copy is frozen). If none changed in the probe
/// window we fall back to the first basis-valid candidate.
fn acquire_lock(proc: &ProcessHandle, slot: u8, shutdown: &Arc<AtomicBool>) -> Option<ScanResult> {
    let candidates = locate_matrix_candidates(proc, slot);

    let mut valid: Vec<(ScanResult, [u8; MATRIX_SIZE_BYTES])> = Vec::new();
    for c in candidates {
        if let Some(bytes) = proc.read_exact::<MATRIX_SIZE_BYTES>(c.matrix_address) {
            if parse_matrix(&bytes).basis_looks_valid() {
                valid.push((c, bytes));
            }
        }
    }

    match valid.len() {
        0 => None,
        1 => Some(valid[0].0),
        _ => {
            sleep_interruptible(LIVENESS_PROBE, shutdown);
            for (res, first) in &valid {
                if let Some(second) = proc.read_exact::<MATRIX_SIZE_BYTES>(res.matrix_address) {
                    if second != *first {
                        return Some(*res);
                    }
                }
            }
            Some(valid[0].0)
        }
    }
}

/// Run the reader until `shutdown` is set. Blocks the calling thread.
pub fn run<S: TelemetrySink>(config: ReaderConfig, shutdown: Arc<AtomicBool>, sink: &mut S) {
    let frame_period = Duration::from_secs_f64(1.0 / config.poll_hz.max(1) as f64);
    let mut deriver = Deriver::new();

    while !shutdown.load(Ordering::Relaxed) {
        // 1) Wait for the game.
        let pid = match find_process_pid() {
            Some(pid) => pid,
            None => {
                sink.on_status(Status::WaitingForGame);
                sleep_interruptible(Duration::from_secs(1), &shutdown);
                continue;
            }
        };

        // 2) Open it (read-only).
        let proc = match ProcessHandle::open(pid) {
            Ok(p) => p,
            Err(_) => {
                sleep_interruptible(Duration::from_millis(500), &shutdown);
                continue;
            }
        };

        // 3) Scan + validate.
        sink.on_status(Status::Scanning);
        let lock = loop {
            if shutdown.load(Ordering::Relaxed) || !proc.is_alive() {
                break None;
            }
            if let Some(res) = acquire_lock(&proc, config.slot, &shutdown) {
                break Some(res);
            }
            // Not found yet (likely still at menus / car not spawned). Retry.
            sleep_interruptible(Duration::from_millis(750), &shutdown);
        };

        let lock = match lock {
            Some(l) => l,
            None => {
                if !proc.is_alive() {
                    sink.on_status(Status::GameClosed);
                }
                deriver.reset();
                continue;
            }
        };

        sink.on_status(Status::Locked {
            matrix_address: lock.matrix_address,
            slot: lock.slot,
        });
        deriver.reset();

        // 4) Poll loop.
        let mut last = Instant::now();
        let mut invalid_streak: u32 = 0;
        let mut alive_check = Instant::now();
        let mut last_bytes: Option<[u8; MATRIX_SIZE_BYTES]> = None;
        let mut static_secs: f32 = 0.0;

        loop {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            let frame_start = Instant::now();

            match proc.read_exact::<MATRIX_SIZE_BYTES>(lock.matrix_address) {
                Some(bytes) => {
                    let transform = parse_matrix(&bytes);
                    if transform.basis_looks_valid() {
                        invalid_streak = 0;
                        let now = Instant::now();
                        // Clamp dt to absorb scheduler hitches without spiking
                        // derived acceleration.
                        let dt = (now - last).as_secs_f32().clamp(0.0005, 0.1);
                        last = now;

                        // Stale-lock guard: a live car (even idling) jitters; a
                        // dead leftover copy is byte-identical forever. If nothing
                        // changes for a while, the lock is stale or the car
                        // despawned to a menu -> re-scan to find the live car.
                        if Some(bytes) == last_bytes {
                            static_secs += dt;
                        } else {
                            static_secs = 0.0;
                            last_bytes = Some(bytes);
                        }
                        if static_secs > STALE_LOCK_SECS {
                            sink.on_status(Status::LockLost);
                            deriver.reset();
                            break;
                        }

                        let frame = deriver.update(&transform, dt);
                        sink.on_frame(&frame);
                    } else {
                        invalid_streak += 1;
                    }
                }
                None => {
                    invalid_streak += 1;
                }
            }

            // Lock went stale (car despawned, address moved) -> re-scan.
            if invalid_streak >= MAX_INVALID_FRAMES {
                sink.on_status(Status::LockLost);
                deriver.reset();
                break;
            }

            // Periodically confirm the process is still alive (~ every 1s).
            if alive_check.elapsed() >= Duration::from_secs(1) {
                alive_check = Instant::now();
                if !proc.is_alive() {
                    sink.on_status(Status::GameClosed);
                    deriver.reset();
                    break;
                }
            }

            // Maintain cadence.
            if let Some(rem) = frame_period.checked_sub(frame_start.elapsed()) {
                sleep_interruptible(rem, &shutdown);
            }
        }
    }
}

/// Sleep that wakes early if shutdown is requested, so Ctrl+C is responsive.
fn sleep_interruptible(dur: Duration, shutdown: &Arc<AtomicBool>) {
    let step = Duration::from_millis(50);
    let mut remaining = dur;
    while remaining > Duration::ZERO {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        let nap = step.min(remaining);
        std::thread::sleep(nap);
        remaining = remaining.saturating_sub(nap);
    }
}
