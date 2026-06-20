//! Memory scanner: locate the `carRootNodeNN` ASCII string in the live process,
//! then resolve the transform-matrix address from it.

use crate::process::ProcessHandle;
use crate::signatures;

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
