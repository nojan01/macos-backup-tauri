//! Additional macOS flags that system tar does not serialize (notably UF_TRACKED).
//! Kept inside the archive, so the archive's SHA-256 also covers these records.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::os::macos::fs::MetadataExt;
use std::os::unix::ffi::OsStrExt;
use std::path::Component;

const NAME: &str = ".macos-backup-suite-flags-v1";
const ALTERNATE: &str = ".macos-backup-suite-flags-v1-alternate";
const OWNER: &str = "macos-backup-flags-v1";
const MAGIC: &[u8] = b"macOS Backup Suite file flags v1\n";
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub path: String,
    pub flags: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Flags {
    pub root: String,
    // New archives encode metadata in PAX headers; ._ names are literal data.
    // Missing on v1.2.17/18 archives, whose AppleDouble handling stays unchanged.
    #[serde(default)]
    pub pax_metadata: bool,
    pub entries: Vec<Record>,
}
fn name(root: &str) -> &'static str {
    if root.eq_ignore_ascii_case(NAME) || root.eq_ignore_ascii_case(&format!("._{NAME}")) {
        ALTERNATE
    } else {
        NAME
    }
}
pub(super) fn write(stage: &Path, flags: &Flags) -> Result<PathBuf, String> {
    let mut bytes = MAGIC.to_vec();
    serde_json::to_writer(&mut bytes, flags).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Dateiflag-Metadaten sind zu groß".into());
    }
    let name = name(&flags.root);
    let path = stage.join("flags-metadata.tar");
    let mut tar = tar::Builder::new(fs::File::create(&path).map_err(|e| e.to_string())?);
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username(OWNER).map_err(|e| e.to_string())?;
    header.set_cksum();
    tar.append_data(&mut header, name, bytes.as_slice())
        .map_err(|e| e.to_string())?;
    tar.finish().map_err(|e| e.to_string())?;
    Ok(path)
}
// A reserved-looking filename alone is not a marker: old archives and actual
// source files with that name remain ordinary data unless the signature matches.
pub(super) fn read<R: Read>(
    path: &Path,
    entry: &mut tar::Entry<'_, R>,
) -> Result<Option<Flags>, String> {
    if !(path == Path::new(NAME) || path == Path::new(ALTERNATE))
        || !entry.header().entry_type().is_file()
    {
        return Ok(None);
    }
    if entry.header().username().map_err(|e| e.to_string())? != Some(OWNER) {
        return Ok(None);
    }
    let mut prefix = Vec::new();
    entry
        .by_ref()
        .take(MAGIC.len() as u64)
        .read_to_end(&mut prefix)
        .map_err(|e| e.to_string())?;
    if prefix != MAGIC {
        return Err("Ungültige Signatur der Dateiflag-Metadaten".into());
    }
    let mut data = Vec::new();
    entry
        .by_ref()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_BYTES {
        return Err("Dateiflag-Metadaten sind zu groß".into());
    }
    let flags: Flags =
        serde_json::from_slice(&data).map_err(|e| format!("Ungültige Dateiflag-Metadaten: {e}"))?;
    if path != Path::new(name(&flags.root)) {
        return Err("Dateiflag-Metadaten haben einen ungültigen Namen".into());
    }
    Ok(Some(flags))
}
pub(super) fn validate(
    flags: &Flags,
    entries: &BTreeMap<PathBuf, tar::EntryType>,
) -> Result<(), String> {
    super::restore::validate_component(&flags.root)?;
    super::restore::require_root(
        &entries.keys().cloned().collect(),
        std::ffi::OsStr::new(&flags.root),
    )?;
    let mut seen = BTreeSet::new();
    for entry in &flags.entries {
        let rel = Path::new(&entry.path);
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) || entry.path.contains('\0')
        {
            return Err(format!(
                "Unsicherer Pfad in Dateiflag-Metadaten: {:?}",
                entry.path
            ));
        }
        let path = Path::new(&flags.root).join(rel);
        if !entries.contains_key(&path) || !seen.insert(path) || entry.flags == 0 {
            return Err(format!(
                "Ungültiger oder doppelter Dateiflag-Eintrag: {:?}",
                entry.path
            ));
        }
    }
    Ok(())
}
pub(super) fn companion(path: &Path) -> PathBuf {
    PathBuf::from(format!("._{}", path.to_string_lossy()))
}

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
pub(super) fn apply(target: &Path, flags: &Flags) -> Result<(), String> {
    let _phase = crate::work_progress::Phase::enter("Gesicherte macOS-Dateiflags wiederherstellen");
    let mut records: Vec<_> = flags.entries.iter().collect();
    // Set directory restrictions only after processing their descendants.
    records.sort_by_key(|r| std::cmp::Reverse(Path::new(&r.path).components().count()));
    for record in records {
        if BACKUP_CANCELLED.load(Ordering::SeqCst) || VERIFY_CANCELLED.load(Ordering::SeqCst) {
            return Err("Vorgang abgebrochen".into());
        }
        let relative = if record.path.is_empty() {
            PathBuf::from(&flags.root)
        } else {
            Path::new(&flags.root).join(&record.path)
        };
        for parent in relative
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            let md = fs::symlink_metadata(target.join(parent)).map_err(|e| e.to_string())?;
            if !md.is_dir() || md.file_type().is_symlink() {
                return Err(
                    "Dateiflags dürfen nicht durch symbolische Links gesetzt werden".into(),
                );
            }
        }
        set(&target.join(relative), record.flags)?;
    }
    Ok(())
}
