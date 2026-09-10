//! Wait for an actual screen unlock when macOS denies protected file access.
//! Never changes file permissions, disables locking, or treats unreadable data as saved.
use std::io;

#[derive(Debug)]
pub(crate) enum AccessError {
    Io(io::Error),
    Cancelled,
}

pub(crate) fn retry<T>(mut operation: impl FnMut() -> io::Result<T>) -> Result<T, AccessError> {
    let mut phase = None;
    retry_with(
        &mut operation,
        screen_locked,
        || {
            crate::BACKUP_CANCELLED.load(std::sync::atomic::Ordering::SeqCst)
                || crate::VERIFY_CANCELLED.load(std::sync::atomic::Ordering::SeqCst)
        },
        || std::thread::sleep(std::time::Duration::from_millis(250)),
        |paused| {
            if paused {
                phase = Some(crate::work_progress::Phase::enter(
                    "Backup pausiert: Mac entsperren, um geschützte Dateien weiterzulesen",
                ));
            } else {
                phase.take();
            }
        },
    )
}

/// A failed tar creation only wrote a private, unpublished archive. Re-running
/// the command truncates that partial file; the source manifest and readback
/// checks still have to pass before anything is published.
pub(crate) fn create_archive(
    mut command: impl FnMut() -> std::process::Command,
) -> Result<std::process::Output, String> {
    let mut last = None;
    let result = retry(|| {
        let output = crate::run_with_timeout(command(), std::time::Duration::from_secs(24 * 3600));
        let denied = output.as_ref().is_ok_and(|out| {
            let stderr = String::from_utf8_lossy(&out.stderr);
            stderr.contains("Operation not permitted") || stderr.contains("Permission denied")
        });
        last = Some(output);
        if denied {
            Err(io::Error::from_raw_os_error(libc::EPERM))
        } else {
            Ok(())
        }
    });
    match result {
        Err(AccessError::Cancelled) => Err("Vorgang abgebrochen".into()),
        _ => last.unwrap_or_else(|| Err("Archivprozess lieferte kein Ergebnis".into())),
    }
}

fn retry_with<T>(
    mut operation: impl FnMut() -> io::Result<T>,
    mut locked: impl FnMut() -> Option<bool>,
    mut cancelled: impl FnMut() -> bool,
    mut wait: impl FnMut(),
    mut paused: impl FnMut(bool),
) -> Result<T, AccessError> {
    loop {
        if cancelled() {
            return Err(AccessError::Cancelled);
        }
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                if error.kind() != io::ErrorKind::PermissionDenied || locked() != Some(true) {
                    return Err(AccessError::Io(error));
                }
                paused(true);
                while locked() == Some(true) {
                    if cancelled() {
                        return Err(AccessError::Cancelled);
                    }
                    wait();
                }
                paused(false);
                // Retry the same operation after unlock. If access still fails
                // while unlocked (or the state is unknown), return the real error.
            }
        }
    }
}

// IOConsoleLocked is read from the console's IORegistry root. An absent or
// differently typed property is unknown, never an assumption that it is locked.
#[cfg(test)]
thread_local! { static REAL_LOCK_STATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
pub(crate) fn enable_real_lock_state() {
    REAL_LOCK_STATE.with(|value| value.set(true));
}
fn screen_locked() -> Option<bool> {
    // Ordinary filesystem regression fixtures intentionally deny permissions;
    // they must remain deterministic regardless of the developer's screen state.
    #[cfg(test)]
    if !REAL_LOCK_STATE.with(|value| value.get()) {
        return Some(false);
    }
    read_screen_locked()
}
fn read_screen_locked() -> Option<bool> {
    use std::ffi::c_void;
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegistryGetRootEntry(port: u32) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const c_void,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const std::ffi::c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFGetTypeID(value: *const c_void) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: *const c_void) -> u8;
        fn CFRelease(value: *const c_void);
    }
    unsafe {
        let root = IORegistryGetRootEntry(0);
        if root == 0 {
            return None;
        }
        let key =
            CFStringCreateWithCString(std::ptr::null(), c"IOConsoleLocked".as_ptr(), 0x08000100);
        if key.is_null() {
            IOObjectRelease(root);
            return None;
        }
        let value = IORegistryEntryCreateCFProperty(root, key, std::ptr::null(), 0);
        CFRelease(key);
        IOObjectRelease(root);
        if value.is_null() {
            return None;
        }
        let result = if CFGetTypeID(value) == CFBooleanGetTypeID() {
            Some(CFBooleanGetValue(value) != 0)
        } else {
            None
        };
        CFRelease(value);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    fn denied() -> io::Error {
        io::Error::from_raw_os_error(libc::EPERM)
    }

    #[test]
    fn denied_access_waits_for_unlock_then_retries_without_skipping() {
        let ticks = Cell::new(0);
        let attempts = Cell::new(0);
        let mut states = Vec::new();
        let value = retry_with(
            || {
                attempts.set(attempts.get() + 1);
                if ticks.get() < 3 {
                    Err(denied())
                } else {
                    Ok(b"actual data")
                }
            },
            || Some(ticks.get() < 3),
            || false,
            || ticks.set(ticks.get() + 1),
            |state| states.push(state),
        )
        .unwrap();
        assert_eq!(value, b"actual data");
        assert_eq!(attempts.get(), 2);
        assert_eq!(ticks.get(), 3);
        assert_eq!(states, vec![true, false]);
    }

    #[test]
    fn permission_denial_after_unlock_is_an_error() {
        let ticks = Cell::new(0);
        let attempts = Cell::new(0);
        let result: Result<(), _> = retry_with(
            || {
                attempts.set(attempts.get() + 1);
                Err(denied())
            },
            || Some(ticks.get() == 0),
            || false,
            || ticks.set(1),
            |_| {},
        );
        assert!(matches!(result, Err(AccessError::Io(_))));
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn cancellation_while_locked_stops_without_retrying_the_read() {
        let ticks = Cell::new(0);
        let attempts = Cell::new(0);
        let result: Result<(), _> = retry_with(
            || {
                attempts.set(attempts.get() + 1);
                Err(denied())
            },
            || Some(true),
            || ticks.get() == 2,
            || ticks.set(ticks.get() + 1),
            |_| {},
        );
        assert!(matches!(result, Err(AccessError::Cancelled)));
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn unlocked_unknown_and_io_errors_do_not_wait() {
        for state in [None, Some(false)] {
            let result: Result<(), _> = retry_with(
                || Err(denied()),
                || state,
                || false,
                || panic!("must not wait"),
                |_| panic!("must not pause"),
            );
            assert!(matches!(result, Err(AccessError::Io(_))));
        }
        let result: Result<(), _> = retry_with(
            || Err(io::Error::from_raw_os_error(libc::EIO)),
            || Some(true),
            || false,
            || panic!("must not wait"),
            |_| panic!("must not pause"),
        );
        assert!(matches!(result, Err(AccessError::Io(_))));
    }

    #[test]
    fn readable_files_continue_while_locked() {
        assert_eq!(
            retry_with(
                || Ok(42),
                || Some(true),
                || false,
                || panic!("must not wait"),
                |_| {}
            )
            .unwrap(),
            42
        );
    }
    #[test]
    #[ignore = "manual read-only screen-lock probe"]
    fn actual_screen_lock_state() {
        println!("SCREEN_LOCKED={:?}", read_screen_locked());
    }

    #[test]
    fn failed_archive_creation_keeps_exit_status_and_stderr() {
        crate::BACKUP_CANCELLED.store(false, std::sync::atomic::Ordering::SeqCst);
        crate::VERIFY_CANCELLED.store(false, std::sync::atomic::Ordering::SeqCst);
        let output = create_archive(|| {
            let mut command = std::process::Command::new("/bin/sh");
            command.args([
                "-c",
                "printf 'fixture: Operation not permitted' >&2; exit 7",
            ]);
            command
        })
        .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stderr, b"fixture: Operation not permitted");
    }
}
