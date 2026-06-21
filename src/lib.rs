//! wreckfest-teleport — automatic telemetry for Wreckfest 1.
//!
//! Wreckfest 1 exposes no telemetry API (no shared memory, no UDP). This crate
//! reads the live process memory, locating the player car's transform matrix via
//! the `carRootNode00` node string (the approach proven by SpaceMonkey, MIT —
//! see NOTICE), and derives real motion telemetry from it: position,
//! orientation, velocity, g-force / surge-sway-heave, angular rates and impact.
//!
//! It deliberately does NOT report RPM, gear, throttle, brake, suspension, tyre
//! or fuel data: Wreckfest 1 does not expose those values in a readable form, so
//! we do not fabricate them. What you get is exactly what the game reveals.
//!
//! ## Library-first
//! All logic lives here so this folds into sim-teleport as `crates/
//! wreckfest-teleport` later with no restructuring. The entry point is
//! [`reader::run`], driven by a [`reader::ReaderConfig`] and a
//! [`reader::TelemetrySink`]. [`emitter::UdpEmitter`] is a ready-made sink.

pub mod emitter;
pub mod math;
pub mod picker;
pub mod process;
pub mod reader;
pub mod scan;
pub mod signatures;
pub mod telemetry;

pub use reader::{run, ReaderConfig, Status, TelemetrySink};
pub use telemetry::{Deriver, Telemetry};

/// Crate version, printed by the binary (`--version`). Matches Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
