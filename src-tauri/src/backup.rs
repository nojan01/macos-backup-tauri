//! Fail-closed source scanning, durable publication and verified archive creation.
use super::*;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::io::Write;
use std::os::unix::{
    ffi::OsStrExt,
    fs::{MetadataExt, OpenOptionsExt},
};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestEntry {
    pub p: String,
    pub s: u64,
    kind: String,
    hash: String,
    link: Option<Vec<u8>>,
    mode: u32,
    uid: u32,
    gid: u32,
    m: i64,
    mn: i64,
    c: i64,
    cn: i64,
    dev: u64,
    ino: u64,
    flags: u32,
    xattrs: BTreeMap<String, String>,
    acl: String,
}

fn fail(path: &Path, e: impl std::fmt::Display) -> String {
    format!("{}: {e}", path.display())
}
fn cancelled() -> Result<(), String> {
    if BACKUP_CANCELLED.load(Ordering::SeqCst) {
        Err("Backup abgebrochen".into())
    } else {
        Ok(())
    }
}
fn identity(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.mode(),
    )
}
fn read_acl(path: &Path) -> Result<String, String> {
    unsafe extern "C" {
        fn acl_get_link_np(path: *const libc::c_char, kind: libc::c_int) -> *mut libc::c_void;
        fn acl_to_text(acl: *mut libc::c_void, len: *mut libc::ssize_t) -> *mut libc::c_char;
        fn acl_free(obj: *mut libc::c_void) -> libc::c_int;
    }
    let name = CString::new(path.as_os_str().as_bytes()).map_err(|e| fail(path, e))?;
    unsafe {
        let acl = acl_get_link_np(name.as_ptr(), 0x100);
        if acl.is_null() {
            let error = std::io::Error::last_os_error();
            // Darwin reports ENOENT for an existing object with no extended ACL.
            if error.raw_os_error() == Some(libc::ENOENT) && fs::symlink_metadata(path).is_ok() {
                return Ok(String::new());
            }
            return Err(fail(path, error));
        }
        let raw = acl_to_text(acl, std::ptr::null_mut());
        if raw.is_null() {
            acl_free(acl);
            return Err(fail(path, std::io::Error::last_os_error()));
        }
        let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
        acl_free(raw.cast());
        acl_free(acl);
        Ok(text)
    }
}

pub(super) fn compute_snapshot(root: &Path) -> Result<Vec<ManifestEntry>, String> {
    cancelled()?;
    if std::env::var("BACKUP_EXTRA_EXCLUDES").is_ok_and(|v| !v.trim().is_empty()) {
        return Err("BACKUP_EXTRA_EXCLUDES wird nicht mehr stillschweigend angewandt. Variable entfernen; alle ausgewählten Daten werden vollständig gesichert.".into());
    }
    let mut entries = Vec::new();
    for dent in WalkDir::new(root)
        .follow_links(false)
        .follow_root_links(false)
    {
        cancelled()?;
        let dent = dent.map_err(|e| fail(root, e))?;
        let path = dent.path();
        let md = fs::symlink_metadata(path).map_err(|e| fail(path, e))?;
        let kind = if md.is_file() {
            "file"
        } else if md.is_dir() {
            "dir"
        } else if md.file_type().is_symlink() {
            "link"
        } else {
            return Err(fail(path,"Nicht unterstützte Spezialdatei (z. B. Socket/FIFO). Quelle vor dem Backup bereinigen oder enger auswählen."));
        };
        let rel = path.strip_prefix(root).map_err(|e| fail(path, e))?;
        let p = rel
            .to_str()
            .ok_or_else(|| fail(path, "Dateiname ist kein gültiges UTF-8"))?
            .to_string();
        let hash = if md.is_file() {
            let mut file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .map_err(|e| fail(path, e))?;
            if identity(&file.metadata().map_err(|e| fail(path, e))?) != identity(&md) {
                return Err(fail(path, "Quelle während des Lesens geändert"));
            }
            let mut digest = Sha256::new();
            let mut buf = vec![0; 1024 * 1024];
            loop {
                cancelled()?;
                let n = file.read(&mut buf).map_err(|e| fail(path, e))?;
                if n == 0 {
                    break;
                }
                digest.update(&buf[..n]);
            }
            format!("{:x}", digest.finalize())
        } else {
            String::new()
        };
        let link = if md.file_type().is_symlink() {
            Some(
                fs::read_link(path)
                    .map_err(|e| fail(path, e))?
                    .as_os_str()
                    .as_bytes()
                    .to_vec(),
            )
        } else {
            None
        };
        let mut xattrs = BTreeMap::new();
        for key in xattr::list(path).map_err(|e| fail(path, e))? {
            let value = xattr::get(path, &key)
                .map_err(|e| fail(path, e))?
                .ok_or_else(|| fail(path, "Dateiattribut während des Lesens entfernt"))?;
            xattrs.insert(
                key.to_str().ok_or("Invalid xattr name")?.to_string(),
                format!("{:x}", Sha256::digest(&value)),
            );
        }
        let acl = read_acl(path)?;
        if identity(&md) != identity(&fs::symlink_metadata(path).map_err(|e| fail(path, e))?) {
            return Err(fail(path, "Quelle während des Lesens geändert"));
        }
        use std::os::macos::fs::MetadataExt as MacMetadataExt;
        entries.push(ManifestEntry {
            p,
            s: if md.is_file() { md.len() } else { 0 },
            kind: kind.into(),
            hash,
            link,
            mode: md.mode(),
            uid: md.uid(),
            gid: md.gid(),
            m: md.mtime(),
            mn: md.mtime_nsec(),
            c: md.ctime(),
            cn: md.ctime_nsec(),
            dev: md.dev(),
            ino: md.ino(),
            flags: md.st_flags(),
            xattrs,
            acl,
        });
    }
    entries.sort_by(|a, b| a.p.cmp(&b.p));
    Ok(entries)
}

pub(super) fn ensure_unchanged(source: &Path, expected: &[ManifestEntry]) -> Result<(), String> {
    if compute_snapshot(source)? != expected {
        return Err(fail(source,"Quelle während der Sicherung verändert. Schreibende Programme schließen und Backup erneut starten."));
    }
    Ok(())
}

pub(super) fn require_free_space(path: &Path, required: u64) -> Result<(), String> {
    let path = super::restore::resolve_existing_ancestor(path)?;
    let mut existing = path.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or("Speicherziel fehlt")?;
    }
    let name = CString::new(existing.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(fail(existing, std::io::Error::last_os_error()));
    }
    let stats = unsafe { stats.assume_init() };
    let available = (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64);
    if available < required.saturating_add(16 * 1024 * 1024) {
        return Err(fail(existing,format!("Nicht genug freier Speicher: {available} Bytes verfügbar, mindestens {required} Bytes plus Reserve erforderlich")));
    }
    Ok(())
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing parent")?;
    let stage = PrivateDir::new(parent, ".write")?;
    let tmp = stage.0.join("new");
    let mut f = fs::File::create(&tmp).map_err(|e| fail(path, e))?;
    f.write_all(bytes).map_err(|e| fail(path, e))?;
    f.sync_all().map_err(|e| fail(path, e))?;
    publish(&tmp, path)
}
fn publish(tmp: &Path, path: &Path) -> Result<(), String> {
    fs::File::open(tmp)
        .and_then(|f| f.sync_all())
        .map_err(|e| fail(path, e))?;
    fs::rename(tmp, path).map_err(|e| fail(path, e))?;
    fs::File::open(path.parent().ok_or("Missing parent")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| fail(path, e))
}
pub(super) fn reuse_archive(source: &Path, target: &Path) -> Result<(), String> {
    let stage = PrivateDir::new(target.parent().ok_or("Missing parent")?, ".reuse")?;
    let tmp = stage.0.join("archive");
    if fs::hard_link(source, &tmp).is_err() {
        fs::copy(source, &tmp).map_err(|e| fail(target, e))?;
    }
    publish(&tmp, target)
}

/// Only owns newly extracted files, never hardlinks to live sources or old backups.
struct ReadbackDir(PrivateDir);
impl Drop for ReadbackDir {
    fn drop(&mut self) {
        if fs::remove_dir_all(&self.0 .0).is_ok() {
            return;
        }
        unsafe extern "C" {
            fn lchflags(path: *const libc::c_char, flags: libc::c_uint) -> libc::c_int;
            fn acl_init(count: libc::c_int) -> *mut libc::c_void;
            fn acl_set_link_np(
                path: *const libc::c_char,
                kind: libc::c_int,
                acl: *mut libc::c_void,
            ) -> libc::c_int;
            fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
        }
        use std::os::unix::fs::PermissionsExt;
        // Read-only modes, immutable flags and deny-delete ACLs from the source
        // must not strand a private extraction indefinitely on the system disk.
        for entry in WalkDir::new(&self.0 .0)
            .follow_links(false)
            .follow_root_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if let Ok(path) = CString::new(entry.path().as_os_str().as_bytes()) {
                unsafe {
                    lchflags(path.as_ptr(), 0);
                    let acl = acl_init(0);
                    if !acl.is_null() {
                        acl_set_link_np(path.as_ptr(), 0x100, acl);
                        acl_free(acl);
                    }
                }
                if !entry.file_type().is_symlink() {
                    let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o700));
                }
            }
        }
        // The contained PrivateDir performs the final removal after this drop.
    }
}

/// Extract and compare actual restored bytes and metadata, rather than trusting tar's exit code.
pub(super) fn verify_archive_source(
    archive: &Path,
    root_name: &str,
    expected: &[ManifestEntry],
) -> Result<(), String> {
    let required = expected
        .iter()
        .map(|e| e.s.saturating_add(4096))
        .sum::<u64>();
    require_free_space(
        &std::env::temp_dir(),
        required.saturating_add(required / 10),
    )?;
    // Use the native local filesystem: archives can live on exFAT/network drives,
    // which cannot represent macOS ACLs and xattrs as ordinary extracted files.
    let owned = ReadbackDir(PrivateDir::temp()?);
    let stage = &owned.0;
    require_root(&archive_index(archive)?, std::ffi::OsStr::new(root_name))?;
    unpack_private(archive, &stage.0)?;
    let actual = compute_snapshot(&stage.0.join(root_name))?;
    if actual.len() != expected.len() {
        return Err(fail(archive, "Archiv enthält nicht alle Quelldateien"));
    }
    // Identity/change times belong to the live filesystem, ownership is deliberately
    // mapped to the restoring user. All restorable content/permissions are compared.
    for (a, b) in actual.iter().zip(expected) {
        if (
            a.p.as_str(),
            a.s,
            &a.kind,
            &a.hash,
            &a.link,
            a.mode,
            a.m,
            a.mn,
            a.flags,
            &a.xattrs,
            &a.acl,
        ) != (
            b.p.as_str(),
            b.s,
            &b.kind,
            &b.hash,
            &b.link,
            b.mode,
            b.m,
            b.mn,
            b.flags,
            &b.xattrs,
            &b.acl,
        ) {
            return Err(fail(
                archive,
                format!("Archiv-Rückleseprüfung fehlgeschlagen: {}", b.p),
            ));
        }
    }
    // Hard links within a source must still share an inode after extraction.
    let mut links = BTreeMap::new();
    for (a, b) in actual
        .iter()
        .zip(expected)
        .filter(|(_, b)| b.kind == "file")
    {
        if let Some(first) = links.insert((b.dev, b.ino), (a.dev, a.ino)) {
            if first != (a.dev, a.ino) {
                return Err(fail(archive, "Hardlink-Beziehung verloren"));
            }
        }
    }
    Ok(())
}

pub(super) fn create_verified_archive(
    source: &Path,
    target: &Path,
    gzip: bool,
) -> Result<(), String> {
    let expected = compute_snapshot(source)?;
    let bytes = expected
        .iter()
        .map(|e| e.s.saturating_add(4096))
        .sum::<u64>();
    require_free_space(
        target.parent().ok_or("Missing parent")?,
        bytes.saturating_add(bytes / 10),
    )?;
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Ungültige Quellwurzel")?;
    let stage = PrivateDir::new(target.parent().ok_or("Missing parent")?, ".archive")?;
    let tmp = stage.0.join("archive");
    // Exact NUL-delimited list; no glob exclusions, no recursive second traversal.
    let mut members = Vec::new();
    for item in &expected {
        members.extend_from_slice(b"./");
        members.extend_from_slice(name.as_bytes());
        if !item.p.is_empty() {
            members.push(b'/');
            members.extend_from_slice(item.p.as_bytes());
        }
        members.push(0);
    }
    let list = stage.0.join("members");
    fs::write(&list, members).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("/usr/bin/tar");
    cmd.current_dir(source.parent().ok_or("Missing source parent")?);
    cmd.args([
        "--format=pax",
        "--acls",
        "--xattrs",
        "--fflags",
        "--no-recursion",
        "--null",
    ]);
    if !gzip && get_zstd_path().is_some() {
        cmd.arg(format!(
            "--use-compress-program={} -T0 -1",
            get_zstd_path().unwrap()
        ))
        .arg("-cf");
    } else {
        cmd.arg("-czf");
    }
    cmd.arg(&tmp).arg("-T").arg(&list);
    let output = run_with_timeout(cmd, std::time::Duration::from_secs(24 * 3600))?;
    require_success("Archive creation", &output)?;
    if !output.stderr.is_empty() {
        return Err(fail(
            source,
            format!("tar meldet: {}", String::from_utf8_lossy(&output.stderr)),
        ));
    }
    ensure_unchanged(source, &expected)?;
    verify_archive_source(&tmp, name, &expected)?;
    ensure_unchanged(source, &expected)?;
    cancelled()?;
    publish(&tmp, target)
}

pub(super) fn validate_source_target(source: &Path, target: &Path) -> Result<(), String> {
    let canonical = fs::canonicalize(source).map_err(|e| fail(source, e))?;
    let resolved = super::restore::resolve_existing_ancestor(target)?;
    if resolved.starts_with(&canonical)
        || canonical.starts_with(resolved.join("macos-backup-suite"))
    {
        return Err("Backup-Ziel und Quelldaten dürfen nicht ineinander liegen".into());
    }
    Ok(())
}

pub(super) fn finish_backup(
    root: &Path,
    metadata: &BackupMetadata,
    sources: &[(PathBuf, Vec<ManifestEntry>)],
) -> Result<(), String> {
    validate_backup_metadata(metadata)?;
    if metadata.items.is_empty() {
        return Err("Keine Daten für das Backup ausgewählt".into());
    }
    for item in &metadata.items {
        verify_item(root, item)?;
    }
    for (source, expected) in sources {
        ensure_unchanged(source, expected)?;
    }
    cancelled()?;
    atomic_write(
        &root.join("metadata.json"),
        &serde_json::to_vec_pretty(metadata).map_err(|e| e.to_string())?,
    )
}

#[cfg(test)]
mod tests;
