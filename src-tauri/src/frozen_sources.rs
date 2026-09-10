//! Read every selected source from the same read-only APFS snapshot, never from
//! a manifest captured long before a mutable live file is eventually archived.
use super::*;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::time::{Duration, Instant};

pub(super) fn homebrew_cache_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/var/homebrew/cache"),
        PathBuf::from("/usr/local/var/homebrew/cache"),
        home.join("Library/Caches/Homebrew"),
    ]
}
pub(super) fn safari_paths(home: &Path) -> Vec<PathBuf> {
    [
        "Library/Safari/Bookmarks.plist",
        "Library/Safari/ReadingListArchives",
        "Library/Safari/Extensions",
        "Library/Preferences/com.apple.Safari.plist",
        "Library/Containers/com.apple.Safari/Data/Library/Preferences",
        "Library/Safari/Favicon Cache",
        "Library/Safari/TopSites.plist",
        "Library/Safari/LastSession.plist",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}

pub(super) fn existing(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let mut result = Vec::new();
    for path in paths {
        match fs::symlink_metadata(&path) {
            Ok(_) => result.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(result)
}

struct Volume {
    root: PathBuf,
    read_only: bool,
}
fn volume(path: &Path) -> Result<Volume, String> {
    let md = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let query = if md.file_type().is_symlink() {
        path.parent().ok_or("Quelle ohne übergeordneten Ordner")?
    } else {
        path
    };
    let name = CString::new(query.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(name.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let stat = unsafe { stat.assume_init() };
    let kind = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) }.to_bytes();
    if kind != b"apfs" {
        return Err(format!(
            "{}: Konsistente Sicherung benötigt eine APFS-Quelle; gefunden {}",
            path.display(),
            String::from_utf8_lossy(kind)
        ));
    }
    let root = PathBuf::from(
        unsafe { CStr::from_ptr(stat.f_mntonname.as_ptr()) }
            .to_str()
            .map_err(|e| e.to_string())?,
    );
    Ok(Volume {
        root,
        read_only: stat.f_flags & libc::MNT_RDONLY as u32 != 0,
    })
}
fn relative_source(path: &Path, root: &Path) -> Result<PathBuf, String> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Unsicherer Quellpfad".into());
    }
    let name = path
        .file_name()
        .ok_or("Eine Dateisystemwurzel kann nicht als Quellordner gesichert werden")?;
    // Resolve parent aliases, while retaining a selected symlink itself.
    let canonical = path
        .parent()
        .ok_or("Quelle ohne Elternordner")?
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(name);
    let rel = if let Ok(rel) = canonical.strip_prefix(root) {
        rel.to_path_buf()
    } else if root == Path::new("/System/Volumes/Data") {
        canonical
            .strip_prefix("/")
            .map_err(|e| e.to_string())?
            .to_path_buf()
    } else {
        return Err(format!(
            "{} liegt nicht auf {}",
            path.display(),
            root.display()
        ));
    };
    // /Users and /opt can be Data-volume firmlinks. Verify the actual object,
    // instead of assuming every absolute path belongs to the Data volume.
    let a = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let b = fs::symlink_metadata(root.join(&rel)).map_err(|e| e.to_string())?;
    if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
        return Err(format!(
            "Snapshot-Zuordnung stimmt nicht: {}",
            path.display()
        ));
    }
    Ok(rel)
}
fn namespace(path: &Path) -> PathBuf {
    if let Ok(rel) = path.strip_prefix("/System/Volumes/Data") {
        Path::new("/").join(rel)
    } else {
        path.to_path_buf()
    }
}
fn nested_mount(source: &Path, mount: &Path) -> bool {
    let source = namespace(source);
    let mount = namespace(mount);
    mount != source && mount.starts_with(source)
}
fn mount_points() -> Result<Vec<PathBuf>, String> {
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut capacity = count as usize + 16;
    for _ in 0..3 {
        let mut buffer: Vec<std::mem::MaybeUninit<libc::statfs>> = Vec::with_capacity(capacity);
        let bytes = capacity
            .checked_mul(std::mem::size_of::<libc::statfs>())
            .and_then(|n| i32::try_from(n).ok())
            .ok_or("Zu viele eingehängte Volumes")?;
        let n = unsafe { libc::getfsstat(buffer.as_mut_ptr().cast(), bytes, libc::MNT_NOWAIT) };
        if n < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if n as usize >= capacity {
            capacity *= 2;
            continue;
        }
        unsafe {
            buffer.set_len(n as usize);
        }
        return buffer
            .iter()
            .map(|m| {
                let m = unsafe { m.assume_init_ref() };
                Ok(PathBuf::from(
                    unsafe { CStr::from_ptr(m.f_mntonname.as_ptr()) }
                        .to_str()
                        .map_err(|e| e.to_string())?,
                ))
            })
            .collect();
    }
    Err("Volume-Liste hat sich während der Snapshot-Vorprüfung wiederholt geändert".into())
}
fn snapshot_name(output: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(output).map_err(|e| e.to_string())?;
    let date = text
        .lines()
        .find_map(|l| l.strip_prefix("Created local snapshot with date: "))
        .ok_or("Time Machine hat keinen neuen Snapshot bestätigt")?;
    if date.len() != 17
        || !date.bytes().enumerate().all(|(i, b)| {
            if [4, 7, 10].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return Err("Ungültiger Snapshot-Zeitstempel".into());
    }
    Ok(format!("com.apple.TimeMachine.{date}.local"))
}
struct Mounted(PathBuf);
impl Drop for Mounted {
    fn drop(&mut self) {
        // Cancellation must not prevent unmounting. Never recursively remove a
        // mount point: a failed unmount must leave the read-only mount intact.
        let mut cmd = Command::new("/sbin/umount");
        cmd.arg(&self.0)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Ok(mut child) = cmd.spawn() {
            let until = Instant::now() + Duration::from_secs(30);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < until => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        if let Err(e) = fs::remove_dir(&self.0) {
            crate::work_progress::detail(
                format!(
                    "Snapshot-Einhängepunkt bleibt erhalten: {} ({e})",
                    self.0.display()
                ),
                true,
            );
        }
    }
}
pub(super) struct FrozenSources {
    pub snapshot: String,
    paths: BTreeMap<PathBuf, PathBuf>,
    // Dropped after every archive and final source guard has finished.
    _mounts: Vec<Mounted>,
}
impl FrozenSources {
    pub fn capture(sources: &[PathBuf]) -> Result<Self, String> {
        let _phase =
            crate::work_progress::Phase::enter("Unveränderlichen APFS-Sicherungsstand erstellen");
        let mounted = mount_points()?;
        let mut volumes = BTreeMap::new();
        let mut relative = Vec::new();
        for source in sources {
            let v = volume(source)?;
            let rel = relative_source(source, &v.root)?;
            if fs::symlink_metadata(source)
                .map_err(|e| e.to_string())?
                .is_dir()
            {
                if let Some(child) = mounted.iter().find(|m| nested_mount(&v.root.join(&rel), m)) {
                    return Err(format!("{} enthält das separat eingehängte Volume {}. Quellen so auswählen, dass keine Volume-Grenze innerhalb eines Quellordners liegt.",source.display(),child.display()));
                }
            }
            relative.push((source.clone(), v.root.clone(), rel));
            volumes.insert(v.root.clone(), v);
        }
        let snapshot = if volumes.values().any(|v| !v.read_only) {
            let mut cmd = Command::new("/usr/bin/tmutil");
            cmd.arg("localsnapshot").env("LC_ALL", "C");
            let output = run_with_timeout(cmd, Duration::from_secs(300))?;
            require_success("APFS-Snapshot erstellen", &output)?;
            snapshot_name(&output.stdout)?
        } else {
            "bereits schreibgeschützte Quelle".into()
        };
        let mut result = Self {
            snapshot,
            paths: BTreeMap::new(),
            _mounts: Vec::new(),
        };
        let mut roots = BTreeMap::new();
        for (root, v) in volumes {
            if v.read_only {
                roots.insert(root.clone(), root);
                continue;
            }
            let mount = PrivateDir::new(&std::env::temp_dir(), "macos-backup-snapshot")?.keep();
            result._mounts.push(Mounted(mount.clone()));
            let mut cmd = Command::new("/sbin/mount_apfs");
            cmd.args(["-o", "ro,nobrowse,noexec,nosuid,nodev", "-s"])
                .arg(&result.snapshot)
                .arg(&root)
                .arg(&mount);
            let output = run_with_timeout(cmd, Duration::from_secs(120))?;
            require_success(
                &format!(
                    "APFS-Snapshot von {} einhängen (Volume muss in Time Machine enthalten sein)",
                    root.display()
                ),
                &output,
            )?;
            if !volume(&mount)?.read_only {
                return Err("Snapshot ist nicht schreibgeschützt; Sicherung abgebrochen".into());
            }
            roots.insert(root, mount);
        }
        for (source, root, rel) in relative {
            let frozen = roots.get(&root).ok_or("Snapshot-Volume fehlt")?.join(rel);
            fs::symlink_metadata(&frozen)
                .map_err(|e| format!("Quelle im Snapshot fehlt: {}: {e}", source.display()))?;
            result.paths.insert(source, frozen);
        }
        crate::work_progress::detail(format!("Dateien werden aus {} gesichert; Änderungen an den Originalen beeinflussen diesen Sicherungsstand nicht.",result.snapshot),true);
        Ok(result)
    }
    pub fn get(&self, source: &Path) -> Result<PathBuf, String> {
        self.paths.get(source).cloned().ok_or_else(|| {
            format!(
                "Quelle gehört nicht zum eingefrorenen Sicherungsstand: {}",
                source.display()
            )
        })
    }
    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({"snapshot":self.snapshot,"sources":self.paths.keys().collect::<Vec<_>>(),"file_consistency":"read-only APFS snapshot","snapshot_lifecycle":"Time Machine manages the purgeable local snapshot; the suite unmounts its private views when finished."})
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_source_discovery_allows_no_existing_paths() {
        let temp = PrivateDir::temp().unwrap();
        let missing = temp.0.join("missing-homebrew-cache");
        assert!(existing(vec![missing]).unwrap().is_empty());
    }

    #[test]
    fn nested_volume_checks_account_for_data_firmlinks_and_component_boundaries() {
        assert!(nested_mount(
            Path::new("/System/Volumes/Data/Users/test"),
            Path::new("/Users/test/mounted")
        ));
        assert!(!nested_mount(
            Path::new("/Users/test"),
            Path::new("/Users/test2/mounted")
        ));
        assert!(!nested_mount(
            Path::new("/Users/test"),
            Path::new("/System/Volumes/Data/Users/test")
        ));
        assert!(mount_points().unwrap().iter().any(|p| p == Path::new("/")));
    }
    #[test]
    fn snapshot_result_requires_a_confirmed_safe_name() {
        assert_eq!(
            snapshot_name(b"Created local snapshot with date: 2026-09-08-141413\n").unwrap(),
            "com.apple.TimeMachine.2026-09-08-141413.local"
        );
        for text in [
            "2026-09-08-141413",
            "Created local snapshot with date: ../../bad",
            "Created local snapshot with date: 2026-09-08-141413;bad",
        ] {
            assert!(snapshot_name(text.as_bytes()).is_err());
        }
    }
    #[test]
    fn data_volume_mapping_preserves_selected_symlinks_and_rejects_parent_steps() {
        let d = PrivateDir::temp().unwrap();
        let file = d.0.join("file");
        fs::write(&file, b"x").unwrap();
        let link = d.0.join("link");
        std::os::unix::fs::symlink("missing", &link).unwrap();
        let v = volume(&file).unwrap();
        let mapped = v.root.join(relative_source(&link, &v.root).unwrap());
        assert!(fs::symlink_metadata(mapped)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(relative_source(&d.0.join("../file"), &v.root).is_err());
    }
}
