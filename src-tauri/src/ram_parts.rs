//! Bounded, disk-free AppleArchive wrapper for one segmented backup part.
//! The outer .aarset format remains identical to the disk-backed implementation.
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::{BACKUP_CANCELLED, VERIFY_CANCELLED};

pub(crate) const PART_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RAW_ARCHIVE: usize = PART_BYTES as usize + 256 * 1024;
const MIN_AVAILABLE: u64 = 2 * 1024 * 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

fn error(s: impl std::fmt::Display) -> io::Error {
    io::Error::other(s.to_string())
}

#[cfg(test)]
thread_local! { static TEST_READY: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
pub(crate) fn force_ready_for_test(value: Option<bool>) {
    TEST_READY.with(|c| c.set(value));
}
/// Query conservatively for every part because available memory changes during a backup.
pub(crate) fn ready() -> bool {
    #[cfg(test)]
    if let Some(value) = TEST_READY.with(|c| c.get()) {
        return value;
    }
    unsafe {
        unsafe extern "C" {
            fn mach_host_self() -> libc::mach_port_t;
        }
        let mut total = 0u64;
        let mut size = std::mem::size_of::<u64>();
        if libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut total as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
            || total < 8 * GIB
        {
            return false;
        }
        let mut vm: libc::vm_statistics64_data_t = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        if libc::host_statistics64(
            mach_host_self(),
            libc::HOST_VM_INFO64,
            (&mut vm as *mut libc::vm_statistics64_data_t).cast(),
            &mut count,
        ) != 0
            || count < libc::HOST_VM_INFO64_COUNT
        {
            return false;
        }
        let page = libc::sysconf(libc::_SC_PAGESIZE);
        if page <= 0 {
            return false;
        }
        // Half of inactive pages may be reclaimed; do not budget all caches.
        let pages = vm.free_count as u64 + vm.inactive_count as u64 / 2 + vm.purgeable_count as u64;
        pages.saturating_mul(page as u64) >= MIN_AVAILABLE
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Key(u32);
fn key(s: &[u8; 3]) -> Key {
    Key(u32::from_le_bytes([s[0], s[1], s[2], 0]))
}
type Header = *mut libc::c_void;
#[link(name = "AppleArchive")]
unsafe extern "C" {
    fn AAHeaderCreate() -> Header;
    fn AAHeaderCreateWithEncodedData(size: usize, data: *const u8) -> Header;
    fn AAHeaderDestroy(h: Header);
    fn AAHeaderSetFieldUInt(h: Header, index: u32, key: Key, value: u64) -> i32;
    fn AAHeaderSetFieldString(
        h: Header,
        index: u32,
        key: Key,
        value: *const libc::c_char,
        length: usize,
    ) -> i32;
    fn AAHeaderSetFieldBlob(h: Header, index: u32, key: Key, size: u64) -> i32;
    fn AAHeaderGetEncodedSize(h: Header) -> usize;
    fn AAHeaderGetEncodedData(h: Header) -> *const u8;
    fn AAHeaderGetFieldCount(h: Header) -> u32;
    fn AAHeaderGetFieldKey(h: Header, index: u32) -> Key;
    fn AAHeaderGetFieldUInt(h: Header, index: u32, value: *mut u64) -> i32;
    fn AAHeaderGetFieldString(
        h: Header,
        index: u32,
        capacity: usize,
        value: *mut libc::c_char,
        length: *mut usize,
    ) -> i32;
    fn AAHeaderGetPayloadSize(h: Header) -> u64;
    fn AAHeaderGetFieldBlob(h: Header, index: u32, size: *mut u64, offset: *mut u64) -> i32;
}
struct OwnedHeader(Header);
impl Drop for OwnedHeader {
    fn drop(&mut self) {
        unsafe { AAHeaderDestroy(self.0) }
    }
}

fn wrapper_header(size: u64) -> io::Result<Vec<u8>> {
    let h = OwnedHeader(unsafe { AAHeaderCreate() });
    if h.0.is_null() {
        return Err(error("AppleArchive-Header konnte nicht erstellt werden"));
    }
    let path = b"payload";
    let success = unsafe {
        AAHeaderSetFieldUInt(h.0, u32::MAX, key(b"TYP"), b'F' as u64) >= 0
            && AAHeaderSetFieldString(h.0, u32::MAX, key(b"PAT"), path.as_ptr().cast(), path.len())
                >= 0
            && AAHeaderSetFieldBlob(h.0, u32::MAX, key(b"DAT"), size) >= 0
    };
    if !success {
        return Err(error("AppleArchive-Header ungültig"));
    }
    let n = unsafe { AAHeaderGetEncodedSize(h.0) };
    let p = unsafe { AAHeaderGetEncodedData(h.0) };
    if n < 6 || n > 65535 || p.is_null() {
        return Err(error("AppleArchive-Header-Größe ungültig"));
    }
    Ok(unsafe { std::slice::from_raw_parts(p, n) }.to_vec())
}

fn payload_from_raw(raw: &[u8], expected_len: u64, expected_hash: &str) -> io::Result<Vec<u8>> {
    if raw.len() < 6 || &raw[..4] != b"AA01" {
        return Err(error("Ungültiges RAM-Teilarchiv"));
    }
    let n = u16::from_le_bytes([raw[4], raw[5]]) as usize;
    if n < 6 || n > raw.len() {
        return Err(error("RAM-Teilarchiv-Größe stimmt nicht"));
    }
    let h = OwnedHeader(unsafe { AAHeaderCreateWithEncodedData(n, raw.as_ptr()) });
    if h.0.is_null() {
        return Err(error("RAM-Teilarchiv-Header ungültig"));
    }
    let payload_bytes = unsafe { AAHeaderGetPayloadSize(h.0) };
    if payload_bytes > expected_len + 131072 || raw.len() as u64 != n as u64 + payload_bytes {
        return Err(error("RAM-Teilarchiv-Nutzdaten ungültig"));
    }
    let mut typ = None;
    let mut path = None;
    let mut dat = None;
    for i in 0..unsafe { AAHeaderGetFieldCount(h.0) } {
        let k = unsafe { AAHeaderGetFieldKey(h.0, i) }.0;
        if k == key(b"TYP").0 {
            let mut v = 0;
            if typ.is_some() || unsafe { AAHeaderGetFieldUInt(h.0, i, &mut v) } < 0 {
                return Err(error("Doppelter oder ungültiger Typ"));
            }
            typ = Some(v);
        } else if k == key(b"PAT").0 {
            let mut v = [0i8; 8];
            let mut length = 0usize;
            if path.is_some()
                || unsafe { AAHeaderGetFieldString(h.0, i, v.len(), v.as_mut_ptr(), &mut length) }
                    < 0
            {
                return Err(error("Ungültiger Teilarchiv-Pfad"));
            }
            path = Some(v[..length].iter().map(|c| *c as u8).collect::<Vec<_>>());
        } else if k == key(b"DAT").0 || k == key(b"XAT").0 || k == key(b"ACL").0 {
            let mut size = 0;
            let mut offset = 0;
            if unsafe { AAHeaderGetFieldBlob(h.0, i, &mut size, &mut offset) } < 0 {
                return Err(error("Ungültige Nutzdaten"));
            }
            if k == key(b"DAT").0 {
                if dat.is_some() {
                    return Err(error("Doppelter Datenblock"));
                }
                dat = Some((size, offset));
            } else if size > 65536 {
                return Err(error("Teilarchiv-Metadaten zu groß"));
            }
        } else if ![
            b"IDX", b"IDZ", b"UID", b"GID", b"MOD", b"FLG", b"MTM", b"CTM", b"BTM", b"SIZ", b"DUZ",
            b"INO", b"SH2",
        ]
        .iter()
        .any(|allowed| k == key(allowed).0)
        {
            return Err(error("Unerwartetes Teilarchiv-Feld"));
        }
    }
    let Some((size, offset)) = dat else {
        return Err(error("Teilarchiv ohne Datenblock"));
    };
    if typ != Some(b'F' as u64)
        || path.as_deref() != Some(b"payload".as_slice())
        || size != expected_len
        || offset
            .checked_add(size)
            .is_none_or(|end| end > payload_bytes)
    {
        return Err(error(
            "Teilarchiv enthält nicht genau die erwarteten Nutzdaten",
        ));
    }
    let start = n + offset as usize;
    let end = start + expected_len as usize;
    let payload = &raw[start..end];
    if format!("{:x}", Sha256::digest(payload)) != expected_hash {
        return Err(error("Entpackte Prüfsumme stimmt nicht"));
    }
    Ok(payload.to_vec())
}

/// A bounded subprocess pipeline. Both pipes are drained concurrently; the
/// stdout cap rejects corrupt archives before they can exhaust memory.
fn convert(input: Vec<u8>, algorithm: &str, cap: usize) -> io::Result<Vec<u8>> {
    let mut child = Command::new("/usr/bin/aa")
        .args(["convert", "-a", algorithm, "-b", "1m", "-t", "2"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| error("AppleArchive stdin fehlt"))?;
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("AppleArchive stdout fehlt"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| error("AppleArchive stderr fehlt"))?;
    let errors = std::thread::spawn(move || {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            if data.len() < 65536 {
                data.extend_from_slice(&buf[..n.min(65536 - data.len())]);
            }
        }
        data
    });
    let mut output = Vec::new();
    let mut chunk = [0u8; 1024 * 1024];
    let started = Instant::now();
    use std::os::fd::AsRawFd;
    let fd = stdout.as_raw_fd();
    let mut problem = None;
    loop {
        if BACKUP_CANCELLED.load(Ordering::SeqCst) || VERIFY_CANCELLED.load(Ordering::SeqCst) {
            problem = Some("Backup abgebrochen".to_owned());
            break;
        }
        if started.elapsed() > Duration::from_secs(600) {
            problem = Some("AppleArchive-Zeitlimit überschritten".to_owned());
            break;
        }
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pollfd, 1, 100) };
        if ready < 0 {
            problem = Some(io::Error::last_os_error().to_string());
            break;
        }
        if ready == 0 {
            continue;
        }
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) if output.len().saturating_add(n) <= cap => output.extend_from_slice(&chunk[..n]),
            Ok(_) => {
                problem = Some("AppleArchive-Ausgabe überschreitet RAM-Grenze".to_owned());
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                problem = Some(e.to_string());
                break;
            }
        }
    }
    if problem.is_some() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let _ = writer.join();
    let details = errors.join().unwrap_or_default();
    if let Some(e) = problem {
        return Err(error(e));
    }
    if !status.success() || !details.is_empty() {
        return Err(error(format!(
            "AppleArchive ({status}): {}",
            String::from_utf8_lossy(&details)
        )));
    }
    Ok(output)
}

pub(crate) fn encode(payload: &[u8]) -> io::Result<Vec<u8>> {
    let header = wrapper_header(payload.len() as u64)?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(header.len() + payload.len())
        .map_err(error)?;
    raw.extend_from_slice(&header);
    raw.extend_from_slice(payload);
    convert(raw, "lzfse", MAX_RAW_ARCHIVE)
}

pub(crate) fn decode(
    compressed: Vec<u8>,
    expected_len: u64,
    expected_hash: &str,
) -> io::Result<Vec<u8>> {
    if compressed.len() < 4 || &compressed[..4] != b"pbze" {
        return Err(error("Teilarchive müssen native LZFSE-Archive sein"));
    }
    if expected_len > PART_BYTES {
        return Err(error("RAM-Teilarchiv zu groß"));
    }
    let raw = convert(compressed, "raw", MAX_RAW_ARCHIVE)?;
    payload_from_raw(&raw, expected_len, expected_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_existing_disk_created_part_with_native_metadata() {
        let dir = crate::restore::PrivateDir::temp().unwrap();
        let payload = dir.0.join("payload");
        let data = b"previously stored archive part";
        std::fs::write(&payload, data).unwrap();
        let archive = dir.0.join("part.aar");
        crate::apple_archive::create(&payload, &archive).unwrap();
        let compressed = std::fs::read(&archive).unwrap();
        let hash = format!("{:x}", Sha256::digest(data));
        assert_eq!(decode(compressed, data.len() as u64, &hash).unwrap(), data);
    }
    #[test]
    fn memory_wrapper_round_trip_and_corruption_rejection() {
        let data = vec![0x5a; 1024 * 1024 + 17];
        let hash = format!("{:x}", Sha256::digest(&data));
        let compressed = encode(&data).unwrap();
        assert_eq!(
            decode(compressed.clone(), data.len() as u64, &hash).unwrap(),
            data
        );
        assert!(decode(compressed, data.len() as u64, &"0".repeat(64)).is_err());
    }
}
