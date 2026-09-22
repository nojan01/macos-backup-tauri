//! Native AppleArchive containers with LZFSE. No tar or external compressor.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Component;
use unicode_normalization::UnicodeNormalization;

const AA: &str = "/usr/bin/aa";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

#[derive(Debug, Deserialize)]
pub(super) struct Entry {
    #[serde(rename = "PAT")]
    pub path: PathBuf,
    #[serde(rename = "TYP")]
    pub kind: String,
    #[serde(rename = "LNK")]
    pub link: Option<String>,
    #[serde(rename = "DAT", default)]
    pub size: u64,
    #[serde(rename = "HLC")]
    pub hardlink: Option<u64>,
    #[serde(rename = "SH2")]
    pub hash: Option<String>,
}
fn normalized(path: &Path) -> String {
    path.to_string_lossy()
        .nfc()
        .collect::<String>()
        .to_lowercase()
}
fn validate(entries: Vec<Entry>) -> Result<BTreeMap<PathBuf, Entry>, String> {
    let mut result = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut clusters = BTreeMap::new();
    for entry in entries {
        let path = &entry.path;
        if path.as_os_str().is_empty()
            || path.to_str().is_none_or(|p| {
                p.split('/')
                    .any(|c| c.is_empty() || c == "." || c == ".." || c.contains('\0'))
            })
            || !path.components().all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(format!("Unsafe AppleArchive path: {}", path.display()));
        }
        if !matches!(entry.kind.as_str(), "D" | "F" | "L") {
            return Err(format!(
                "Unsupported AppleArchive entry: {} ({})",
                path.display(),
                entry.kind
            ));
        }
        if (entry.kind == "L") != entry.link.is_some() {
            return Err(format!(
                "Invalid AppleArchive symbolic link: {}",
                path.display()
            ));
        }
        if let Some(cluster) = entry.hardlink {
            if entry.kind != "F" {
                return Err("Hardlink cluster on non-file entry".into());
            }
            let identity = (entry.size, entry.hash.clone());
            if clusters
                .insert(cluster, identity.clone())
                .is_some_and(|prior| prior != identity)
            {
                return Err("Conflicting AppleArchive hardlink cluster".into());
            }
        }
        if names.insert(normalized(path), entry.kind.clone()).is_some() {
            return Err(format!(
                "Archive names collide on macOS: {}",
                path.display()
            ));
        }
        result.insert(path.clone(), entry);
    }
    if result.is_empty() {
        return Err("Archive contains no items".into());
    }
    for path in result.keys() {
        for parent in path
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            if !names
                .get(&normalized(parent))
                .is_some_and(|kind| kind == "D")
            {
                return Err(format!(
                    "Archive has missing or non-directory ancestor: {}",
                    parent.display()
                ));
            }
        }
    }
    Ok(result)
}
fn read_command(mut cmd: Command, archive: &Path) -> Result<std::process::Output, String> {
    let reader = crate::throttle::open_throttled(archive).map_err(|e| e.to_string())?;
    cmd.env("LC_ALL", "C");
    crate::run_with_timeout_stdin(cmd, TIMEOUT, Box::new(reader))
}
pub(super) fn inspect(archive: &Path) -> Result<BTreeMap<PathBuf, Entry>, String> {
    let _phase = crate::work_progress::Phase::enter("AppleArchive-Struktur prüfen");
    let mut magic = [0u8; 4];
    fs::File::open(archive)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|e| e.to_string())?;
    // Apple's LZFSE archive stream (pbze), not a filename-based format guess.
    if &magic != b"pbze" {
        return Err("Dieses Backup benötigt AppleArchive mit LZFSE (.aar). TAR-Archive werden in dieser Version nicht unterstützt.".into());
    }
    let mut cmd = Command::new(AA);
    cmd.args(["list", "-list-format", "json"]);
    let output = read_command(cmd, archive)?;
    require_success("AppleArchive index", &output)?;
    if !output.stderr.is_empty() {
        return Err(format!(
            "AppleArchive index: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let entries = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Ungültiger AppleArchive-Index: {e}"))?;
    validate(entries)
}
pub(super) fn index(archive: &Path) -> Result<BTreeSet<PathBuf>, String> {
    Ok(inspect(archive)?.into_keys().collect())
}
pub(super) fn create(source: &Path, target: &Path) -> Result<(), String> {
    let _phase = crate::work_progress::Phase::enter(&format!(
        "AppleArchive mit LZFSE erstellen: {}",
        source.display()
    ));
    let source_metadata = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    // aa's explicit single-input mode resolves symlinks. Stage only the link
    // with copyfile(NO FOLLOW), then enumerate that private one-entry directory.
    let link_stage = if source_metadata.file_type().is_symlink() {
        let stage = crate::backup::ReadbackDir(PrivateDir::temp()?);
        let copy = stage
            .0
             .0
            .join(source.file_name().ok_or("Missing source name")?);
        unsafe extern "C" {
            fn copyfile(
                from: *const libc::c_char,
                to: *const libc::c_char,
                state: *mut libc::c_void,
                flags: u32,
            ) -> libc::c_int;
        }
        let from = CString::new(source.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let to = CString::new(copy.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        // COPYFILE_ALL | COPYFILE_NOFOLLOW_SRC | COPYFILE_NOFOLLOW_DST
        if unsafe {
            copyfile(
                from.as_ptr(),
                to.as_ptr(),
                std::ptr::null_mut(),
                15 | (1 << 18) | (1 << 19),
            )
        } != 0
        {
            return Err(format!(
                "Symlink sichern: {}: {}",
                source.display(),
                std::io::Error::last_os_error()
            ));
        }
        crate::archive_flags::set(
            &copy,
            std::os::macos::fs::MetadataExt::st_flags(&source_metadata),
        )?;
        Some(stage)
    } else {
        None
    };
    let result = crate::protected_access::create_archive(target, || {
        let mut cmd = Command::new(AA);
        cmd.arg("archive");
        if let Some(stage) = &link_stage {
            cmd.arg("-d")
                .arg(&stage.0 .0)
                .args(["-exclude-regex", "^$"]);
        } else if source_metadata.is_dir() {
            cmd.arg("-D").arg(source);
        } else {
            cmd.arg("-d")
                .arg(source.parent().unwrap_or(Path::new(".")))
                .arg("-i")
                .arg(source.file_name().unwrap_or_default());
        }
        cmd.args([
            "-a",
            "lzfse",
            "-include-field",
            "attr,xat,acl,sh2",
            "-exclude-field",
            "uid,gid",
            "-exclude-type",
            "s",
            "-x",
            "-o",
        ])
        .arg(target)
        .env("LC_ALL", "C");
        cmd
    })
    .and_then(|output| {
        require_success("AppleArchive creation", &output)?;
        if !output.stderr.is_empty() {
            return Err(format!(
                "AppleArchive meldet: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    })
    .map_err(|error| {
        format!(
            "Quelle: {}; Archivziel: {}: {error}",
            source.display(),
            target.display()
        )
    });
    crate::work_progress::report_result(result)
}
pub(super) fn extract(
    archive: &Path,
    target: &Path,
    root: Option<&std::ffi::OsStr>,
) -> Result<(), String> {
    let entries = inspect(archive)?;
    if let Some(root) = root {
        require_root(&entries.keys().cloned().collect(), root)?;
    }
    let md = fs::symlink_metadata(target).map_err(|e| e.to_string())?;
    if !md.is_dir()
        || md.file_type().is_symlink()
        || fs::read_dir(target)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err("Extraction requires an empty private directory".into());
    }
    let required = entries
        .values()
        .try_fold(2u64 * 1024 * 1024 * 1024, |sum, e| {
            sum.checked_add(e.size).ok_or("Archive size overflow")
        })?;
    crate::backup::require_free_space(target, required)?;
    let _phase = crate::work_progress::Phase::enter(
        "AppleArchive entpacken und macOS-Metadaten wiederherstellen",
    );
    let mut cmd = Command::new(AA);
    cmd.args(["extract", "-d"]).arg(target).args([
        "-exclude-field",
        "uid,gid",
        "-no-ignore-eperm",
        "-enable-holes",
    ]);
    let output = read_command(cmd, archive)?;
    crate::work_progress::report_result(require_success("AppleArchive extraction", &output))?;
    if !output.stderr.is_empty() {
        return Err(format!(
            "AppleArchive meldet: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checked(json: serde_json::Value) -> Result<BTreeMap<PathBuf, Entry>, String> {
        validate(serde_json::from_value(json).unwrap())
    }
    #[test]
    fn index_rejects_traversal_duplicates_special_files_and_link_ancestors() {
        for path in ["../escape", "/absolute", "root/../escape", ""] {
            assert!(checked(serde_json::json!([{"TYP":"F","PAT":path}])).is_err());
        }
        for entries in [
            serde_json::json!([{"TYP":"D","PAT":"root"},{"TYP":"F","PAT":"Root"}]),
            serde_json::json!([{"TYP":"D","PAT":"root"},{"TYP":"L","PAT":"root/link","LNK":"/tmp"},{"TYP":"F","PAT":"root/link/file"}]),
            serde_json::json!([{"TYP":"D","PAT":"root"},{"TYP":"P","PAT":"root/fifo"}]),
            serde_json::json!([{"TYP":"F","PAT":"root/missing/file"}]),
            serde_json::json!([{"TYP":"D","PAT":"root"},{"TYP":"F","PAT":"root/a","HLC":1,"DAT":4},{"TYP":"F","PAT":"root/b","HLC":1,"DAT":5}]),
        ] {
            assert!(checked(entries).is_err());
        }
    }
    #[test]
    fn non_native_archives_are_rejected_before_extraction() {
        let d = PrivateDir::temp().unwrap();
        let archive = d.0.join("old.aar");
        let output = PrivateDir::temp().unwrap();
        for magic in [[0x1f, 0x8b, 0x08, 0], [0x28, 0xb5, 0x2f, 0xfd]] {
            fs::write(&archive, magic).unwrap();
            assert!(extract(&archive, &output.0, None)
                .unwrap_err()
                .contains("TAR-Archive"));
            assert!(fs::read_dir(&output.0).unwrap().next().is_none());
        }
    }
    #[test]
    fn index_preserves_literal_newlines_and_external_symlink_targets() {
        assert!(checked(serde_json::json!([{"TYP":"D","PAT":"root"},{"TYP":"F","PAT":"root/new\nline"},{"TYP":"L","PAT":"root/link","LNK":"/outside"}])).is_ok());
    }
}
