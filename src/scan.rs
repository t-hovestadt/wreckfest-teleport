//! Memory scanner: locate the `carRootNodeNN` ASCII string in the live process,
//! then resolve the transform-matrix address from it.

use crate::process::ProcessHandle;
use crate::signatures;

/// How much to read per chunk while scanning a region (8 MiB).
const CHUNK: usize = 8 * 1024 * 1024;

/// Scan all readable regions for `needle`. Returns the absolute address of the
/// first match, or None.
fn find_pattern(proc: &ProcessHandle, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }

    let mut result: Option<usize> = None;
    let overlap = needle.len() - 1;
    let mut buf = vec![0u8; CHUNK];

    proc.for_each_readable_region(|base, size| {
        if result.is_some() {
            return;
        }
        let end = base + size;
        let mut cur = base;
        while cur < end {
            let want = CHUNK.min(end - cur);
            let read = proc.read(cur, &mut buf[..want]);

            if read >= needle.len() {
                if let Some(pos) = memmem(&buf[..read], needle) {
                    result = Some(cur + pos);
                    return;
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

    result
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

/// Locate the player's car transform. Scans for `carRootNodeNN`, then applies
/// the backward offset to reach the matrix start. `back_offset` lets the caller
/// retry with the experimental alternate offset if the primary fails validation.
pub fn locate_matrix(proc: &ProcessHandle, slot: u8, back_offset: usize) -> Option<ScanResult> {
    let needle = signatures::node_pattern(slot);
    let string_addr = find_pattern(proc, &needle)?;
    let matrix_address = string_addr.checked_sub(back_offset)?;
    Some(ScanResult {
        matrix_address,
        slot,
    })
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
