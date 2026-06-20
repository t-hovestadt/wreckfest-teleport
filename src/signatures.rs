//! Wreckfest 1 memory signatures.
//!
//! Every value here was extracted verbatim from the SpaceMonkey reference
//! implementation (PHARTGAMES/SpaceMonkey, MIT), file
//! `GenericTelemetryProvider/WreckfestTelemetryProvider.cs`. Nothing here is
//! guessed. See the NOTICE file for attribution.
//!
//! ## How WF1 telemetry is located
//!
//! Wreckfest 1 (2018) exposes no telemetry API: no shared memory, no UDP. The
//! only data path is reading the live process memory. SpaceMonkey's approach
//! (which we port) is:
//!
//! 1. Scan the process for the ASCII string `carRootNode00`. Each car keeps a
//!    named node; `00` is the player's slot. In single-player the local car is
//!    always slot `00`, so this is a constant — no manual selection needed.
//!    (SpaceMonkey: `scanString = "carRootNode" + vehicleString`, ASCII,
//!    `MemoryScanner.StartScanForString` uses `Encoding.ASCII.GetBytes`.)
//!
//! 2. From the address where that string is found, step *backwards*
//!    `((4*4*4)*2)+8` = 136 bytes to reach the start of a 4x4 float transform
//!    matrix. (SpaceMonkey: `memoryAddress = e.MemoryAddresses[0] -
//!    (((4 * 4 * 4) * 2) + 8)`.)
//!
//! 3. Read 64 bytes there = sixteen f32, a row-major 4x4 transform. Position is
//!    the translation row; orientation is the upper-left 3x3. Everything else
//!    (velocity, g-force, surge/sway/heave, angular rates) is derived from this
//!    transform sampled over time.

/// The per-car node name prefix kept in WF1 memory. The player's slot index is
/// appended as a two-digit, zero-padded number.
pub const CAR_NODE_PREFIX: &str = "carRootNode";

/// Single-player local car slot. Always `0` in SP (the only human joiner).
pub const DEFAULT_PLAYER_SLOT: u8 = 0;

/// Bytes to step *backwards* from the found node-string address to reach the
/// start of the transform matrix. SpaceMonkey ships this value in its main
/// provider: `((4*4*4)*2)+8`.
pub const MATRIX_BACK_OFFSET: usize = ((4 * 4 * 4) * 2) + 8; // = 136

/// Experimental alternate offset from SpaceMonkey's
/// `WreckfestTelemetryProviderExperiments.cs` (`(4*4*4)+4`). Kept only as a
/// documented fallback to try if `MATRIX_BACK_OFFSET` ever fails the lock
/// validation on a different build. Not used by default.
pub const MATRIX_BACK_OFFSET_ALT: usize = (4 * 4 * 4) + 4; // = 68

/// Size of the transform matrix in bytes: 4x4 f32.
pub const MATRIX_SIZE_BYTES: usize = 4 * 4 * 4; // = 64

/// Number of f32 in the transform matrix.
pub const MATRIX_FLOATS: usize = 16;

/// Build the ASCII byte pattern to scan for, e.g. slot 0 -> b"carRootNode00".
pub fn node_pattern(slot: u8) -> Vec<u8> {
    format!("{CAR_NODE_PREFIX}{slot:02}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_match_spacemonkey() {
        assert_eq!(MATRIX_BACK_OFFSET, 136);
        assert_eq!(MATRIX_BACK_OFFSET_ALT, 68);
        assert_eq!(MATRIX_SIZE_BYTES, 64);
    }

    #[test]
    fn pattern_is_zero_padded_ascii() {
        assert_eq!(node_pattern(0), b"carRootNode00");
        assert_eq!(node_pattern(8), b"carRootNode08");
        assert_eq!(node_pattern(23), b"carRootNode23");
    }
}
