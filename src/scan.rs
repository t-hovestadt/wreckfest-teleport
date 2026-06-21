//! Memory scanner: locate the `carRootNodeNN` ASCII string in the live process,
//! then resolve the transform-matrix address from it.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::math::Transform;
use crate::process::ProcessHandle;
use crate::signatures::{self, MATRIX_SIZE_BYTES};

/// How much to read per chunk while scanning a region (8 MiB).
const CHUNK: usize = 8 * 1024 * 1024;

/// Scan all readable regions for every occurrence of `needle`, up to
/// `max_matches`. Wreckfest can leave stale copies of the node string in freed
/// memory after a race change, so we must collect all candidates and let the
/// caller pick the live one rather than blindly taking the first.
fn find_all_patterns(proc: &ProcessHandle, needle: &[u8], max_matches: usize) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    if needle.is_empty() || max_matches == 0 {
        return out;
    }

    let overlap = needle.len() - 1;
    let mut buf = vec![0u8; CHUNK];

    proc.for_each_readable_region(|base, size| {
        if out.len() >= max_matches {
            return;
        }
        let end = base + size;
        let mut cur = base;
        while cur < end {
            let want = CHUNK.min(end - cur);
            let read = proc.read(cur, &mut buf[..want]);

            if read >= needle.len() {
                let mut start = 0usize;
                while let Some(rel) = memmem(&buf[start..read], needle) {
                    out.push(cur + start + rel);
                    if out.len() >= max_matches {
                        return;
                    }
                    start += rel + 1;
                    if start >= read {
                        break;
                    }
                }
            }

            if read < want {
                // Hit an unreadable page partway through the region. Skip ahead
                // so we always make progress (at least one page).
                cur += if read == 0 { 0x1000 } else { read };
                continue;
            }

            // Full chunk read: advance but re-include the last `overlap` bytes so
            // a match straddling the chunk boundary is not missed.
            let advance = (want - overlap.min(want.saturating_sub(1))).max(1);
            cur += advance;
        }
    });

    // The overlap re-read can surface the same boundary match twice.
    out.sort_unstable();
    out.dedup();
    out
}

/// Minimal substring search (Rust has no std slice::find for &[u8]).
fn memmem(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    let last_start = haystack.len() - needle.len();
    let mut i = 0;
    while i <= last_start {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Result of a successful scan: the resolved matrix address and the slot used.
#[derive(Clone, Copy, Debug)]
pub struct ScanResult {
    pub matrix_address: usize,
    pub slot: u8,
}

/// Build the list of candidate transform addresses for a slot: every
/// `carRootNodeNN` occurrence, each combined with the primary and the
/// experimental back-offset. The caller validates the basis and probes liveness
/// to choose the real, live car among any stale leftovers.
pub fn locate_matrix_candidates(proc: &ProcessHandle, slot: u8) -> Vec<ScanResult> {
    let needle = signatures::node_pattern(slot);
    let hits = find_all_patterns(proc, &needle, 8);
    let mut out = Vec::new();
    for string_addr in hits {
        for &back in &[
            signatures::MATRIX_BACK_OFFSET,
            signatures::MATRIX_BACK_OFFSET_ALT,
        ] {
            if let Some(matrix_address) = string_addr.checked_sub(back) {
                out.push(ScanResult {
                    matrix_address,
                    slot,
                });
            }
        }
    }
    out
}

/// Liveness-probe gap, mirroring the reader's lock validation.
const PROBE_GAP: Duration = Duration::from_millis(80);

/// Find *every* car in the session in a single memory pass.
///
/// Scanning per-slot (up to 24x) would be unusably slow, so we scan once for the
/// shared `carRootNode` prefix, read the two trailing ASCII digits to recover
/// each slot, resolve its matrix address, and keep one validated, *live* copy
/// per slot. Wreckfest leaves stale duplicates after a race change, so when a
/// slot has several candidates we read each twice a short gap apart and prefer
/// the one whose bytes change (a live car jitters; a dead copy is frozen).
pub fn locate_all_cars(proc: &ProcessHandle) -> Vec<ScanResult> {
    let prefix = signatures::CAR_NODE_PREFIX.as_bytes();
    let hits = find_all_patterns(proc, prefix, 256);

    let mut by_slot: BTreeMap<u8, Vec<usize>> = BTreeMap::new();
    for string_addr in hits {
        let mut digits = [0u8; 2];
        if proc.read(string_addr + prefix.len(), &mut digits) != 2 {
            continue;
        }
        if !digits[0].is_ascii_digit() || !digits[1].is_ascii_digit() {
            continue;
        }
        let slot = (digits[0] - b'0') * 10 + (digits[1] - b'0');
        if let Some(matrix_address) = string_addr.checked_sub(signatures::MATRIX_BACK_OFFSET) {
            by_slot.entry(slot).or_default().push(matrix_address);
        }
    }

    let mut out = Vec::new();
    for (slot, addrs) in by_slot {
        let mut valid: Vec<(usize, [u8; MATRIX_SIZE_BYTES])> = Vec::new();
        for a in addrs {
            if let Some(bytes) = proc.read_exact::<MATRIX_SIZE_BYTES>(a) {
                if Transform::from_le_bytes(&bytes).basis_looks_valid() {
                    valid.push((a, bytes));
                }
            }
        }
        let chosen = match valid.len() {
            0 => None,
            1 => Some(valid[0].0),
            _ => {
                std::thread::sleep(PROBE_GAP);
                let mut pick = valid[0].0;
                for (a, first) in &valid {
                    if let Some(second) = proc.read_exact::<MATRIX_SIZE_BYTES>(*a) {
                        if second != *first {
                            pick = *a;
                            break;
                        }
                    }
                }
                Some(pick)
            }
        };
        if let Some(matrix_address) = chosen {
            out.push(ScanResult { matrix_address, slot });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memmem_finds_and_misses() {
        assert_eq!(memmem(b"xxcarRootNode00yy", b"carRootNode00"), Some(2));
        assert_eq!(memmem(b"nothing here", b"carRootNode00"), None);
        assert_eq!(memmem(b"", b"a"), None);
    }
}
