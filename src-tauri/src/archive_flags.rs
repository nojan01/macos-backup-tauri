//! Restore macOS flags during staged directory merges.
use super::*;
use std::os::macos::fs::MetadataExt;
use std::os::unix::ffi::OsStrExt;

pub(super) fn set(path: &Path, expected: u32) -> Result<(), String> {
    let before = fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .st_flags();
    if before == expected {
        return Ok(());
    }
    // Compression/dataless and protected system flags describe kernel-managed
    // state. Never fake that state by merely flipping its flag on restored bytes.
    let writable = libc::UF_NODUMP
        | libc::UF_IMMUTABLE
        | libc::UF_APPEND
        | libc::UF_OPAQUE
        | libc::UF_TRACKED
        | libc::UF_HIDDEN;
    if (before ^ expected) & !writable != 0 {
        return Err(format!("{}: macOS-Systemflags wurden nicht wiederhergestellt (erwartet 0x{expected:08x}, vorhanden 0x{before:08x})",path.display()));
    }
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    unsafe extern "C" {
        fn lchflags(path: *const libc::c_char, flags: libc::c_uint) -> libc::c_int;
    }
    if unsafe { lchflags(cpath.as_ptr(), expected) } != 0 {
        return Err(format!(
            "{}: Dateiflags 0x{expected:08x} wiederherstellen: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let actual = fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .st_flags();
    if actual != expected {
        return Err(format!("{}: Dateiflags stimmen nach Wiederherstellung nicht überein (erwartet 0x{expected:08x}, vorhanden 0x{actual:08x})",path.display()));
    }
    Ok(())
}
