//! Finding the Wreckfest process and reading its memory.
//!
//! Read-only access only (`PROCESS_VM_READ | PROCESS_QUERY_INFORMATION`). We
//! never write to or inject into the game. Single-player / offline use only.

use std::io;

/// Process names we accept (lowercased before compare). WF1's 64-bit binary is
/// `Wreckfest_x64.exe`. SpaceMonkey matches any process containing "Wreckfest".
pub const PROCESS_NEEDLES: &[&str] = &["wreckfest_x64", "wreckfest"];

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Memory::{
        VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READ,
        PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY,
        PAGE_READWRITE, PAGE_WRITECOPY,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    fn pe_name_to_string(name: &[u16]) -> String {
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..end])
    }

    /// Find the first running process whose name matches one of `PROCESS_NEEDLES`.
    /// Returns its PID, or None if not running.
    pub fn find_process_pid() -> Option<u32> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return None;
            }

            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            let mut found: Option<u32> = None;
            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    let name = pe_name_to_string(&entry.szExeFile).to_lowercase();
                    if PROCESS_NEEDLES.iter().any(|needle| name.contains(needle)) {
                        found = Some(entry.th32ProcessID);
                        break;
                    }
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }

            CloseHandle(snapshot);
            found
        }
    }

    /// An opened, read-only handle to the Wreckfest process.
    pub struct ProcessHandle {
        handle: HANDLE,
        pub pid: u32,
    }

    impl ProcessHandle {
        pub fn open(pid: u32) -> io::Result<Self> {
            unsafe {
                let handle = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid);
                if handle == 0 {
                    return Err(io::Error::from_raw_os_error(GetLastError() as i32));
                }
                Ok(Self { handle, pid })
            }
        }

        /// Read `buf.len()` bytes at `address`. Returns the number of bytes read.
        /// Returns Ok(0) for unreadable pages rather than erroring, so scans can
        /// continue past gaps.
        pub fn read(&self, address: usize, buf: &mut [u8]) -> usize {
            unsafe {
                let mut read: usize = 0;
                let ok = ReadProcessMemory(
                    self.handle,
                    address as *const c_void,
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len(),
                    &mut read,
                );
                // On ERROR_PARTIAL_COPY the call returns FALSE but still writes
                // the bytes it managed to read into `read`, so trust `read`
                // either way (it is pre-initialised to 0 for total failures).
                let _ = ok;
                read
            }
        }

        /// Read exactly `N` bytes, or None if the full read did not succeed.
        pub fn read_exact<const N: usize>(&self, address: usize) -> Option<[u8; N]> {
            let mut buf = [0u8; N];
            if self.read(address, &mut buf) == N {
                Some(buf)
            } else {
                None
            }
        }

        /// Is the process still alive? Cheap liveness check via re-scan of the
        /// snapshot for our PID.
        pub fn is_alive(&self) -> bool {
            unsafe {
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
                if snapshot == INVALID_HANDLE_VALUE {
                    return true; // can't tell; assume alive rather than false-kill
                }
                let mut entry: PROCESSENTRY32W = std::mem::zeroed();
                entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                let mut alive = false;
                if Process32FirstW(snapshot, &mut entry) != 0 {
                    loop {
                        if entry.th32ProcessID == self.pid {
                            alive = true;
                            break;
                        }
                        if Process32NextW(snapshot, &mut entry) == 0 {
                            break;
                        }
                    }
                }
                CloseHandle(snapshot);
                alive
            }
        }

        /// Enumerate committed, readable memory regions and call `f(base, size)`
        /// for each. Skips guard/no-access pages. Used by the scanner.
        pub fn for_each_readable_region<F: FnMut(usize, usize)>(&self, mut f: F) {
            unsafe {
                let readable = PAGE_READONLY
                    | PAGE_READWRITE
                    | PAGE_WRITECOPY
                    | PAGE_EXECUTE_READ
                    | PAGE_EXECUTE_READWRITE
                    | PAGE_EXECUTE_WRITECOPY;
                let blocked = PAGE_GUARD | PAGE_NOACCESS;

                let mut address: usize = 0;
                loop {
                    let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                    let got = VirtualQueryEx(
                        self.handle,
                        address as *const c_void,
                        &mut mbi,
                        std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                    );
                    if got == 0 {
                        break;
                    }

                    let base = mbi.BaseAddress as usize;
                    let size = mbi.RegionSize;

                    if mbi.State == MEM_COMMIT
                        && (mbi.Protect & readable) != 0
                        && (mbi.Protect & blocked) == 0
                    {
                        f(base, size);
                    }

                    let next = base.wrapping_add(size);
                    if next <= address || size == 0 {
                        break; // no forward progress; stop
                    }
                    address = next;
                }
            }
        }
    }

    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            unsafe {
                if self.handle != 0 {
                    CloseHandle(self.handle);
                }
            }
        }
    }
}

// Non-Windows fallback so the crate still type-checks off-Windows. These never
// succeed; the real implementation is Windows-only by nature.
#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn find_process_pid() -> Option<u32> {
        None
    }

    pub struct ProcessHandle {
        pub pid: u32,
    }

    impl ProcessHandle {
        pub fn open(_pid: u32) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "wreckfest-teleport reads process memory and only runs on Windows",
            ))
        }
        pub fn read(&self, _address: usize, _buf: &mut [u8]) -> usize {
            0
        }
        pub fn read_exact<const N: usize>(&self, _address: usize) -> Option<[u8; N]> {
            None
        }
        pub fn is_alive(&self) -> bool {
            false
        }
        pub fn for_each_readable_region<F: FnMut(usize, usize)>(&self, _f: F) {}
    }
}

pub use imp::{find_process_pid, ProcessHandle};
