//! Discover mounted targets through macOS, including mounts outside /Volumes.
//! Keep a backup from writing into the directory beneath a vanished mount.
use std::ffi::{CStr, CString, OsString};
use std::fs;
use std::os::unix::{
    ffi::{OsStrExt, OsStringExt},
    fs::MetadataExt,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mount {
    pub path: PathBuf,
    pub source: String,
    pub fs_type: String,
    pub dev: u64,
    pub network: bool,
    pub read_only: bool,
}

fn field(bytes: &[libc::c_char]) -> String {
    unsafe { CStr::from_ptr(bytes.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn mount_from_stat(stat: &libc::statfs) -> Result<Mount, String> {
    let path = PathBuf::from(OsString::from_vec(
        unsafe { CStr::from_ptr(stat.f_mntonname.as_ptr()) }
            .to_bytes()
            .to_vec(),
    ));
    let fs_type = field(&stat.f_fstypename);
    let source = field(&stat.f_mntfromname);
    let flags = stat.f_flags as u32;
    let dev = fs::metadata(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .dev();
    let remote_type = matches!(fs_type.as_str(), "nfs" | "smbfs" | "webdav")
        || fs_type.contains("fuse")
        || fs_type.contains("rclone");
    Ok(Mount {
        path,
        source,
        fs_type,
        dev,
        network: flags & libc::MNT_LOCAL as u32 == 0 || remote_type,
        read_only: flags & libc::MNT_RDONLY as u32 != 0,
    })
}

pub(crate) fn mounted() -> Result<Vec<Mount>, String> {
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut capacity = count as usize + 16;
    for _ in 0..3 {
        let mut entries: Vec<std::mem::MaybeUninit<libc::statfs>> = Vec::with_capacity(capacity);
        let bytes = capacity
            .checked_mul(std::mem::size_of::<libc::statfs>())
            .and_then(|n| i32::try_from(n).ok())
            .ok_or("Zu viele eingehängte Volumes")?;
        let found =
            unsafe { libc::getfsstat(entries.as_mut_ptr().cast(), bytes, libc::MNT_NOWAIT) };
        if found < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if found as usize >= capacity {
            capacity *= 2;
            continue;
        }
        unsafe { entries.set_len(found as usize) };
        return Ok(entries
            .iter()
            .filter_map(|entry| mount_from_stat(unsafe { entry.assume_init_ref() }).ok())
            .collect());
    }
    Err("Volume-Liste hat sich wiederholt geändert".into())
}

fn at(path: &Path) -> Result<Mount, String> {
    let name = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(name.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(format!(
            "{}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    mount_from_stat(&unsafe { stat.assume_init() })
}

pub(crate) struct Guard {
    target: PathBuf,
    identity: Mount,
}

impl Guard {
    pub(crate) fn new(
        target: &Path,
        selected: &Path,
        expected_source: &str,
    ) -> Result<Self, String> {
        if !target.is_dir()
            || !selected.is_absolute()
            || !target.starts_with(selected)
            || target
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || !target
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(selected.canonicalize().map_err(|e| e.to_string())?)
        {
            return Err("Backup-Ziel muss auf dem ausgewählten, eingehängten Volume liegen".into());
        }
        let identity = at(target)?;
        // A stale /Volumes directory must never become a backup on the Mac's
        // system disk. For custom remote mounts, the saved source identity
        // catches the same fall-through even when the mount path is elsewhere.
        if !selected.starts_with(&identity.path)
            || (selected.starts_with("/Volumes") && !identity.path.starts_with("/Volumes"))
            || (!expected_source.is_empty() && expected_source != identity.source)
        {
            return Err(format!(
                "Backup-Ziel ist nicht wie ausgewählt eingehängt: {} (aktuell: {} auf {})",
                selected.display(),
                identity.source,
                identity.path.display()
            ));
        }
        if identity.read_only {
            return Err(format!(
                "Backup-Ziel ist schreibgeschützt: {}",
                selected.display()
            ));
        }
        Ok(Self {
            target: target.to_path_buf(),
            identity,
        })
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        let current = at(&self.target)?;
        if current.path != self.identity.path
            || current.source != self.identity.source
            || current.fs_type != self.identity.fs_type
            || current.dev != self.identity.dev
        {
            return Err(format!(
                "Backup-Ziel wurde getrennt oder ersetzt: {}",
                self.target.display()
            ));
        }
        Ok(())
    }

    pub(crate) fn is_network(&self) -> bool {
        self.identity.network
    }
    pub(crate) fn description(&self) -> String {
        format!(
            "{} ({}, {})",
            self.identity.path.display(),
            self.identity.fs_type,
            self.identity.source
        )
    }
}

/// A tiny real write/read/rename probe detects unsupported mounted filesystems
/// before the long source scan. The backup uses these operations for publication.
pub(crate) struct Probe {
    pub hardlinks: bool,
}

pub(crate) fn probe(target: &Path) -> Result<Probe, String> {
    use std::io::Write;
    let base = target.join(format!(
        ".macos-backup-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ));
    fs::create_dir(&base).map_err(|e| format!("Schreibprobe {}: {e}", target.display()))?;
    let result = (|| -> Result<Probe, String> {
        let first = base.join("write");
        let second = base.join("renamed");
        let mut file = fs::File::create(&first).map_err(|e| e.to_string())?;
        file.write_all(b"macos-backup-target-test")
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&first, &second).map_err(|e| e.to_string())?;
        let bytes = fs::read(&second).map_err(|e| e.to_string())?;
        if bytes != b"macos-backup-target-test" {
            return Err("Schreibprobe konnte die gespeicherten Daten nicht zurücklesen".into());
        }
        let link = base.join("hardlink");
        let hardlinks = fs::hard_link(&second, &link).is_ok();
        if hardlinks {
            fs::remove_file(&link).map_err(|e| e.to_string())?;
        }
        fs::remove_file(&second).map_err(|e| e.to_string())?;
        fs::File::open(&base)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(Probe { hardlinks })
    })();
    let cleanup = fs::remove_dir_all(&base);
    let capabilities = result
        .map_err(|e| format!("Backup-Ziel unterstützt benötigte Dateioperationen nicht: {e}"))?;
    cleanup.map_err(|e| format!("Schreibprobe konnte nicht entfernt werden: {e}"))?;
    Ok(capabilities)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn write_probe_roundtrips_and_cleans_up() {
        let path = std::env::temp_dir().join(format!("mbs-probe-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        probe(&path).unwrap();
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        fs::remove_dir(&path).unwrap();
    }
}
