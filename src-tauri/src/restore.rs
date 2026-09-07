//! Restore primitives. All archive content is validated and staged before touching live files.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self};
use std::os::unix::fs::DirBuilderExt;
use std::path::Component;
use std::sync::atomic::AtomicU64;
use unicode_normalization::UnicodeNormalization;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub(super) struct PrivateDir(pub PathBuf);
impl PrivateDir {
    pub fn new(parent: &Path, prefix: &str) -> Result<Self, String> {
        for _ in 0..100 {
            let id = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let p = parent.join(format!(
                "{}-{}-{}-{}",
                prefix,
                std::process::id(),
                Local::now().timestamp_nanos_opt().unwrap_or_default(),
                id
            ));
            match fs::DirBuilder::new().mode(0o700).create(&p) {
                Ok(()) => return Ok(Self(p)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("Cannot create staging directory: {e}")),
            }
        }
        Err("Cannot allocate staging directory".into())
    }
    pub fn temp() -> Result<Self, String> {
        Self::new(&std::env::temp_dir(), "macos-backup")
    }
    pub fn keep(self) -> PathBuf {
        let p = self.0.clone();
        std::mem::forget(self);
        p
    }
}
impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn validate_component(value: &str) -> Result<(), String> {
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\', '\0']) {
        return Err(format!("Unsafe path component: {value:?}"));
    }
    Ok(())
}

pub(super) fn archive_name_for(source: &Path, extension: &str) -> String {
    use std::os::unix::ffi::OsStrExt;
    let digest = Sha256::digest(source.as_os_str().as_bytes());
    let name: String = source
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    format!("{}-{:x}.{}", name, digest, extension)
}

pub(super) fn verify_item(backup: &Path, item: &BackupItem) -> Result<(), String> {
    validate_component(&item.archive)?;
    if item.hash.len() != 64 || !item.hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("Invalid SHA-256 for {}", item.path));
    }
    let p = backup.join(&item.archive);
    let md = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", item.archive))?;
    if !md.is_file() || md.file_type().is_symlink() {
        return Err(format!("Not a regular archive: {}", item.archive));
    }
    if md.len() != item.archive_size_bytes {
        return Err(format!("Archive size mismatch: {}", item.archive));
    }
    if !hash_file(&p)?.eq_ignore_ascii_case(&item.hash) {
        return Err(format!("SHA-256 mismatch: {}", item.archive));
    }
    Ok(())
}

struct ArchiveInput {
    reader: Box<dyn Read>,
    child: Option<std::process::Child>,
}
impl Read for ArchiveInput {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if BACKUP_CANCELLED.load(Ordering::SeqCst) || VERIFY_CANCELLED.load(Ordering::SeqCst) {
            // Interrupted would be retried automatically by io::copy and could loop forever.
            return Err(io::Error::other("Vorgang abgebrochen"));
        }
        self.reader.read(b)
    }
}
impl Drop for ArchiveInput {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn compression(archive: &Path) -> Result<bool, String> {
    let mut f = fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut magic = [0; 4];
    f.read_exact(&mut magic).map_err(|e| e.to_string())?;
    if magic[..2] == [0x1f, 0x8b] {
        Ok(false)
    } else if magic == [0x28, 0xb5, 0x2f, 0xfd] {
        Ok(true)
    } else {
        Err("Unsupported or corrupt archive compression".into())
    }
}
fn open_archive(archive: &Path) -> Result<ArchiveInput, String> {
    if compression(archive)? {
        let zstd = get_zstd_path().ok_or("zstd required to read this backup")?;
        let mut child = Command::new(zstd)
            .args(["-d", "-c", "--"])
            .arg(archive)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let reader = Box::new(child.stdout.take().ok_or("No decompressor output")?);
        Ok(ArchiveInput {
            reader,
            child: Some(child),
        })
    } else {
        let f = fs::File::open(archive).map_err(|e| e.to_string())?;
        Ok(ArchiveInput {
            reader: Box::new(flate2::read::MultiGzDecoder::new(f)),
            child: None,
        })
    }
}
fn relative_path(path: &Path) -> Result<PathBuf, String> {
    let mut result = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(n) => result.push(n),
            Component::CurDir => (),
            _ => return Err(format!("Unsafe archive path: {}", path.display())),
        }
    }
    Ok(result)
}
/// Read actual tar headers, not a newline-delimited listing (filenames may contain newlines).
pub(super) fn archive_index(archive: &Path) -> Result<BTreeSet<PathBuf>, String> {
    let mut input = open_archive(archive)?;
    let mut entries = BTreeMap::new();
    let mut hardlinks = Vec::new();
    let mut normalized_names = BTreeSet::new();
    {
        let mut tar = tar::Archive::new(&mut input);
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = relative_path(&entry.path().map_err(|e| e.to_string())?)?;
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir() || kind.is_symlink() || kind.is_hard_link()) {
                return Err(format!("Unsupported archive entry: {}", path.display()));
            }
            if path.as_os_str().is_empty() {
                if kind.is_dir() {
                    continue;
                }
                return Err("Empty archive path".into());
            }
            // macOS destinations can be case-insensitive and Unicode-normalizing.
            if !normalized_names.insert(normalized_path(&path)) {
                return Err(format!(
                    "Archive names collide on macOS: {}",
                    path.display()
                ));
            }
            if entries.insert(path.clone(), kind).is_some() {
                return Err(format!("Duplicate archive entry: {}", path.display()));
            }
            if kind.is_hard_link() {
                let link = entry
                    .link_name()
                    .map_err(|e| e.to_string())?
                    .ok_or("Missing hardlink target")?;
                hardlinks.push((path.clone(), relative_path(&link)?));
            }
            io::copy(&mut entry, &mut io::sink()).map_err(|e| format!("Corrupt archive: {e}"))?;
        }
    }
    // Validate compression trailers/checksums as well as tar headers.
    io::copy(&mut input, &mut io::sink()).map_err(|e| format!("Corrupt archive: {e}"))?;
    if let Some(child) = input.child.as_mut() {
        if !child.wait().map_err(|e| e.to_string())?.success() {
            return Err("Decompression failed".into());
        }
    }
    let normalized_entries: BTreeMap<_, _> = entries
        .iter()
        .map(|(p, k)| (normalized_path(p), *k))
        .collect();
    for path in entries.keys() {
        for parent in path.ancestors().skip(1) {
            if normalized_entries
                .get(&normalized_path(parent))
                .is_some_and(|k| !k.is_dir())
            {
                return Err(format!(
                    "Archive writes through a non-directory: {}",
                    parent.display()
                ));
            }
        }
    }
    for (path, link) in hardlinks {
        if !entries.get(&link).is_some_and(|k| k.is_file()) {
            return Err(format!("Unsafe hardlink: {}", path.display()));
        }
    }
    if entries.is_empty() {
        return Err("Archive contains no items".into());
    }
    Ok(entries.keys().cloned().collect())
}
fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

fn root_matches(path: &Path, root: &std::ffi::OsStr) -> bool {
    let Some(first) = path.components().next() else {
        return false;
    };
    if first
        .as_os_str()
        .to_string_lossy()
        .nfc()
        .eq(root.to_string_lossy().nfc())
    {
        return true;
    }
    // bsdtar uses AppleDouble siblings to retain macOS extended attributes.
    first.as_os_str() == std::ffi::OsString::from(format!("._{}", root.to_string_lossy()))
        && path.components().count() == 1
}
pub(super) fn require_root(
    index: &BTreeSet<PathBuf>,
    root: &std::ffi::OsStr,
) -> Result<(), String> {
    if !index.iter().all(|p| root_matches(p, root))
        || !index.iter().any(|p| {
            p.components().next().is_some_and(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .nfc()
                    .eq(root.to_string_lossy().nfc())
            })
        })
    {
        return Err(format!(
            "Archive root does not match selected target: {}",
            root.to_string_lossy()
        ));
    }
    Ok(())
}

/// Only call on a new, empty, private directory. Live targets use staged_restore.
pub(super) fn unpack_private(archive: &Path, target: &Path) -> Result<(), String> {
    archive_index(archive)?;
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
    let archive = archive.canonicalize().map_err(|e| e.to_string())?;
    let mut cmd = Command::new("/usr/bin/tar");
    if compression(&archive)? {
        cmd.arg(format!(
            "--use-compress-program={} -d",
            get_zstd_path().ok_or("zstd required")?
        ));
        cmd.arg("-xpf");
    } else {
        cmd.arg("-xzpf");
    }
    cmd.arg(&archive).arg("--no-same-owner").current_dir(target);
    let output = run_with_timeout(cmd, std::time::Duration::from_secs(3600))?;
    require_success("Archive extraction", &output)?;
    Ok(())
}

#[derive(Default, Debug)]
pub(super) struct MergeResult {
    pub restored: usize,
    pub skipped: usize,
}
impl MergeResult {
    fn add(&mut self, r: Self) {
        self.restored += r.restored;
        self.skipped += r.skipped;
    }
}
/// Reject destination symlink ancestors. Only macOS's fixed system aliases are resolved.
pub(super) fn safe_parent(target: &Path) -> Result<PathBuf, String> {
    if !target.is_absolute() {
        return Err("Restore target must be absolute".into());
    }
    let parent = target.parent().ok_or("Cannot restore filesystem root")?;
    let mut walked = PathBuf::new();
    for part in parent.components() {
        match part {
            Component::RootDir | Component::Normal(_) => walked.push(part.as_os_str()),
            _ => return Err("Invalid restore target".into()),
        }
        match fs::symlink_metadata(&walked) {
            Ok(md) if md.file_type().is_symlink() => {
                if [Path::new("/tmp"), Path::new("/var"), Path::new("/etc")]
                    .contains(&walked.as_path())
                {
                    walked = walked.canonicalize().map_err(|e| e.to_string())?;
                } else {
                    return Err(format!("Restore parent is a symlink: {}", walked.display()));
                }
            }
            Ok(md) if !md.is_dir() => {
                return Err(format!(
                    "Restore parent is not a directory: {}",
                    walked.display()
                ))
            }
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&walked).map_err(|e| e.to_string())?;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(walked)
}
fn destination_metadata(p: &Path) -> Result<Option<fs::Metadata>, String> {
    match fs::symlink_metadata(p) {
        Ok(md) => Ok(Some(md)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
/// Inspect all conflicts before starting a merge. Never descend through destination links.
pub(super) fn check_merge(source: &Path, target: &Path, overwrite: bool) -> Result<(), String> {
    let src = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if let Some(dst) = destination_metadata(target)? {
        if src.is_dir() && dst.is_dir() && !dst.file_type().is_symlink() {
            for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
                let e = entry.map_err(|e| e.to_string())?;
                check_merge(&e.path(), &target.join(e.file_name()), overwrite)?;
            }
        } else if overwrite && (src.is_dir() || dst.is_dir()) {
            return Err(format!("Directory/type conflict at {}", target.display()));
        }
    }
    Ok(())
}
fn copy_directory_metadata(source: &Path, target: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn copyfile(
            from: *const libc::c_char,
            to: *const libc::c_char,
            state: *mut libc::c_void,
            flags: u32,
        ) -> libc::c_int;
    }
    let from = std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let to = std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // COPYFILE_METADATA | COPYFILE_NOFOLLOW, without copying data or children.
    if unsafe {
        copyfile(
            from.as_ptr(),
            to.as_ptr(),
            std::ptr::null_mut(),
            7 | (1 << 18) | (1 << 19),
        )
    } != 0
    {
        return Err(format!(
            "Cannot restore directory metadata {}: {}",
            target.display(),
            io::Error::last_os_error()
        ));
    }
    Ok(())
}
pub(super) fn merge_tree(
    source: &Path,
    target: &Path,
    overwrite: bool,
) -> Result<MergeResult, String> {
    if BACKUP_CANCELLED.load(Ordering::SeqCst) || VERIFY_CANCELLED.load(Ordering::SeqCst) {return Err("Vorgang abgebrochen".into());}
    let src = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if let Some(dst) = destination_metadata(target)? {
        if src.is_dir() && dst.is_dir() && !dst.file_type().is_symlink() {
            let mut result = MergeResult::default();
            for e in fs::read_dir(source).map_err(|e| e.to_string())? {
                let e = e.map_err(|e| e.to_string())?;
                result.add(merge_tree(
                    &e.path(),
                    &target.join(e.file_name()),
                    overwrite,
                )?);
            }
            if overwrite {
                copy_directory_metadata(source, target)?;
                // Moving the staged children changed the staging directory's mtime.
                let times = fs::FileTimes::new()
                    .set_modified(src.modified().map_err(|e| e.to_string())?)
                    .set_accessed(src.accessed().map_err(|e| e.to_string())?);
                fs::File::open(target)
                    .and_then(|f| f.set_times(times))
                    .map_err(|e| e.to_string())?;
            }
            return Ok(result);
        }
        if !overwrite {
            return Ok(MergeResult {
                restored: 0,
                skipped: 1,
            });
        }
        if src.is_dir() || dst.is_dir() {
            return Err(format!("Directory/type conflict at {}", target.display()));
        }
    }
    // Source is staged on the destination filesystem: rename preserves metadata and
    // atomically replaces a leaf, including dangling symlinks, without following it.
    fs::rename(source, target).map_err(|e| format!("Cannot restore {}: {e}", target.display()))?;
    Ok(MergeResult {
        restored: 1,
        skipped: 0,
    })
}
pub(super) fn staged_restore(
    archive: &Path,
    target: &Path,
    overwrite: bool,
) -> Result<MergeResult, String> {
    let root = target.file_name().ok_or("Cannot restore filesystem root")?;
    staged_restore_named(archive, target, overwrite, root)
}
fn staged_restore_named(
    archive: &Path,
    target: &Path,
    overwrite: bool,
    root: &std::ffi::OsStr,
) -> Result<MergeResult, String> {
    require_root(&archive_index(archive)?, root)?;
    let parent = safe_parent(target)?;
    let target = parent.join(target.file_name().ok_or("Invalid target")?);
    let stage = PrivateDir::new(&parent, ".macos-backup-restore")?;
    unpack_private(archive, &stage.0)?;
    let source = stage.0.join(root);
    check_merge(&source, &target, overwrite)?;
    merge_tree(&source, &target, overwrite)
}
fn whole_home_root(index: &BTreeSet<PathBuf>) -> Result<std::ffi::OsString, String> {
    let root = index
        .iter()
        .filter_map(|p| p.components().next())
        .map(|c| c.as_os_str())
        .find(|n| !n.to_string_lossy().starts_with("._"))
        .ok_or("Home archive has no root")?;
    require_root(index, root)?;
    Ok(root.to_os_string())
}

pub(super) fn require_success(label: &str, output: &std::process::Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{} failed ({}): {} {}",
        label,
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}
pub(super) fn extension_ids(content: &str) -> Result<Vec<String>, String> {
    content
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(|line| {
            let id = line.trim();
            let parts: Vec<_> = id.split('.').collect();
            if parts.len() != 2
                || parts.iter().any(|p| {
                    p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                Err(format!("Invalid VS Code extension ID: {id:?}"))
            } else {
                Ok(id.to_string())
            }
        })
        .collect()
}

pub(super) fn read_inventory(archive: &Path, name: &str) -> Result<String, String> {
    require_root(&archive_index(archive)?, std::ffi::OsStr::new(name))?;
    let stage = PrivateDir::temp()?;
    unpack_private(archive, &stage.0)?;
    let path = stage.0.join(name);
    if !fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err(format!("Inventory is not a regular file: {name}"));
    }
    fs::read_to_string(path).map_err(|e| e.to_string())
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct BrewEntry {
    pub kind: String,
    pub name: String,
}
/// Treat Brewfile as data; never evaluate Ruby or shell content from a backup.
pub(super) fn brew_entries(content: &str) -> Result<Vec<BrewEntry>, String> {
    let mut entries = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((kind, rest)) = line.split_once(' ') else {
            return Err(format!("Unsupported Brewfile line: {line:?}"));
        };
        if kind == "mas" || kind == "vscode" {
            continue;
        } // separate restore items
        if !["brew", "cask", "tap"].contains(&kind) {
            return Err(format!("Unsupported Brewfile entry: {kind}"));
        }
        let rest = rest.trim();
        let quote = rest.chars().next().ok_or("Missing package name")?;
        if quote != '"' && quote != '\'' {
            return Err("Package name must be quoted".into());
        }
        let tail = &rest[1..];
        let end = tail.find(quote).ok_or("Unterminated package name")?;
        let name = &tail[..end];
        if name.is_empty()
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"@._+/-".contains(&b))
            || name.split('/').any(|s| s == ".." || s.is_empty())
        {
            return Err(format!("Invalid package name: {name:?}"));
        }
        // Bundle options are deliberately not executed; installations use the package name.
        // Only accept a syntactic options suffix; reject statements after the string.
        let suffix = tail[end + 1..].trim();
        if !suffix.is_empty() && !suffix.starts_with(',') && !suffix.starts_with('#') {
            return Err("Unsupported Brewfile syntax".into());
        }
        entries.push(BrewEntry {
            kind: kind.into(),
            name: name.into(),
        });
    }
    Ok(entries)
}
pub(super) fn mas_ids(content: &str) -> Result<Vec<String>, String> {
    let mut ids = BTreeSet::new();
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let line = line.trim();
        let id = line
            .strip_prefix("mas ")
            .and_then(|l| l.rsplit_once("id:").map(|(_, id)| id.trim()))
            .ok_or("Invalid MAS inventory line")?;
        if id.is_empty()
            || id.len() > 20
            || !id.bytes().all(|b| b.is_ascii_digit())
            || id.bytes().all(|b| b == b'0')
        {
            return Err("Invalid MAS app ID".into());
        }
        ids.insert(id.into());
    }
    Ok(ids.into_iter().collect())
}
fn special_root(index: &BTreeSet<PathBuf>, names: &[&str]) -> Result<String, String> {
    for name in names {
        if require_root(index, std::ffi::OsStr::new(name)).is_ok() {
            return Ok(name.to_string());
        }
    }
    Err(format!(
        "Unexpected archive root; expected {}",
        names.join(" or ")
    ))
}
pub(super) fn preflight_restore<'a>(
    backup: &Path,
    meta: &'a BackupMetadata,
    items: &[String],
) -> Result<Vec<&'a BackupItem>, String> {
    validate_backup_metadata(meta)?;
    if items.is_empty() {
        return Err("No restore items selected".into());
    }
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for path in items {
        if !seen.insert(path) {
            return Err("Duplicate restore selection".into());
        }
        let item = meta
            .items
            .iter()
            .find(|i| &i.path == path)
            .ok_or_else(|| format!("Not in backup: {path}"))?;
        verify_item(backup, item)?;
        let a = backup.join(&item.archive);
        let index = archive_index(&a)?;
        match path.as_str() {
            "homebrew-packages" => {
                brew_entries(&read_inventory(&a, "homebrew_packages.txt")?)?;
            }
            "mas-apps" => {
                mas_ids(&read_inventory(&a, "mas_apps.txt")?)?;
            }
            "vscode-extensions" => {
                extension_ids(&read_inventory(&a, "vscode_extensions.txt")?)?;
            }
            "safari-settings" => {
                special_root(&index, &["safari_backup"])?;
            }
            "homebrew-cache" => {
                special_root(&index, &["Homebrew", "cache"])?;
            }
            _ => {
                if path == "~" {
                    whole_home_root(&index)?;
                    selected.push(item);
                    continue;
                }
                require_root(
                    &index,
                    Path::new(path).file_name().ok_or("Invalid restore path")?,
                )?;
            }
        }
        selected.push(item);
    }
    Ok(selected)
}
pub(super) fn test_restore_to(
    backup: &Path,
    item_path: &str,
    dest: &Path,
) -> Result<TestRestoreResult, String> {
    let meta = load_backup_metadata(&backup.join("metadata.json"))?;
    if [
        "homebrew-packages",
        "mas-apps",
        "vscode-extensions",
        "safari-settings",
        "homebrew-cache",
    ]
    .contains(&item_path)
    {
        return Err("Select a file/directory archive for Test-Restore".into());
    }
    let items = vec![item_path.to_string()];
    let selected = preflight_restore(backup, &meta, &items)?;
    let item = selected[0];
    let dest = dest.canonicalize().map_err(|e| e.to_string())?;
    if !dest.is_dir() {
        return Err("Destination is not a directory".into());
    }
    if dest.starts_with(backup.canonicalize().map_err(|e| e.to_string())?) {
        return Err("Destination must be outside the backup".into());
    }
    let stage = PrivateDir::new(&dest, "test-restore")?;
    unpack_private(&backup.join(&item.archive), &stage.0)?;
    let mut bytes = 0;
    let mut count = 0;
    for entry in WalkDir::new(&stage.0).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().is_file() {
            bytes += entry.metadata().map_err(|e| e.to_string())?.len();
            count += 1;
        }
    }
    Ok(TestRestoreResult {
        item_path: item_path.into(),
        archive: item.archive.clone(),
        dest_dir: dest.to_string_lossy().into(),
        extracted_path: stage.keep().to_string_lossy().into(),
        bytes_extracted: bytes,
        file_count: count,
    })
}

pub(super) fn restore_selected(
    backup: &Path,
    items: &[String],
    overwrite: bool,
    home: &Path,
    window: Option<&tauri::Window>,
) -> Result<RestoreResult, String> {
    let meta = load_backup_metadata(&backup.join("metadata.json"))?;
    // All hashes, roots and executable inventories must pass before any restore writes.
    let selected = preflight_restore(backup, &meta, items)?;
    let mut result = RestoreResult {
        restored_count: 0,
        skipped_count: 0,
        error_count: 0,
        restored: vec![],
        skipped: vec![],
        errors: vec![],
    };
    for (i, item) in selected.iter().enumerate() {
        if BACKUP_CANCELLED.load(Ordering::SeqCst) || VERIFY_CANCELLED.load(Ordering::SeqCst) {return Err("Vorgang abgebrochen".into());}
        if let Some(w) = window {
            let _=w.emit("restore-progress",serde_json::json!({"progress":i*100/selected.len(),"message":format!("Restoring {}",item.path)}));
        }
        let outcome = match item.path.as_str() {
            "homebrew-packages" => {
                restore_homebrew_packages(backup, &item.archive, overwrite, window).map(|n| {
                    MergeResult {
                        restored: n,
                        skipped: usize::from(n == 0),
                    }
                })
            }
            "mas-apps" => {
                restore_mas_apps(backup, &item.archive, overwrite, window).map(|n| MergeResult {
                    restored: n,
                    skipped: usize::from(n == 0),
                })
            }
            "vscode-extensions" => {
                restore_vscode_extensions(backup, &item.archive, overwrite).map(|n| MergeResult {
                    restored: n,
                    skipped: usize::from(n == 0),
                })
            }
            "safari-settings" => restore_safari_settings(backup, &item.archive, home, overwrite),
            "homebrew-cache" => restore_homebrew_cache(backup, &item.archive, home, overwrite),
            _ => {
                let target = if item.path == "~" {
                    home.to_path_buf()
                } else if let Some(suffix) = item.path.strip_prefix("~/") {
                    home.join(suffix)
                } else if Path::new(&item.path).is_absolute() {
                    PathBuf::from(&item.path)
                } else {
                    home.join(&item.path)
                };
                let backup_abs = backup.canonicalize().map_err(|e| e.to_string())?;
                // Reject overlap before staging; never restore into the backup or an ancestor.
                let target_abs = resolve_existing_ancestor(&target)?;
                if target_abs.starts_with(&backup_abs) || backup_abs.starts_with(&target_abs) {
                    Err("Restore target overlaps backup".into())
                } else {
                    if item.path == "~" {
                        let archive = backup.join(&item.archive);
                        staged_restore_named(
                            &archive,
                            &target,
                            overwrite,
                            &whole_home_root(&archive_index(&archive)?)?,
                        )
                    } else {
                        staged_restore(&backup.join(&item.archive), &target, overwrite)
                    }
                }
            }
        };
        match outcome {
            Ok(outcome) => {
                if outcome.restored > 0 {
                    result.restored.push(item.path.clone());
                }
                if outcome.skipped > 0 || outcome.restored == 0 {
                    result.skipped.push(format!(
                        "{}: {} existing entries skipped",
                        item.path, outcome.skipped
                    ));
                }
            }
            Err(e) => result.errors.push(format!("{}: {e}", item.path)),
        }
    }
    result.restored_count = result.restored.len();
    result.skipped_count = result.skipped.len();
    result.error_count = result.errors.len();
    if let Some(w) = window {
        let _=w.emit("restore-progress",serde_json::json!({"progress":100,"message":if result.errors.is_empty(){"Restore completed"}else{"Restore completed with errors"}}));
    }
    Ok(result)
}
pub(super) fn resolve_existing_ancestor(target: &Path) -> Result<PathBuf, String> {
    if target.exists() {
        return target.canonicalize().map_err(|e| e.to_string());
    }
    let parent = target.parent().ok_or("Invalid restore target")?;
    Ok(resolve_existing_ancestor(parent)?.join(target.file_name().ok_or("Invalid target name")?))
}

/// Copy a validated subtree to staging on the destination filesystem, retaining macOS attributes.
pub(super) fn copy_then_merge(
    source: &Path,
    target: &Path,
    overwrite: bool,
) -> Result<MergeResult, String> {
    let parent = safe_parent(target)?;
    let target = parent.join(target.file_name().ok_or("Invalid target")?);
    check_merge(source, &target, overwrite)?;
    let stage = PrivateDir::new(&parent, ".macos-backup-merge")?;
    let copied = stage.0.join("item");
    let mut cmd = Command::new("/usr/bin/ditto");
    cmd.arg(source).arg(&copied);
    require_success(
        "Copy restore data",
        &run_with_timeout(cmd, std::time::Duration::from_secs(3600))?,
    )?;
    merge_tree(&copied, &target, overwrite)
}
pub(super) fn install_brew_entries(
    brew: &str,
    entries: &[BrewEntry],
    reinstall: bool,
    window: Option<&tauri::Window>,
) -> Result<usize, String> {
    let mut count = 0;
    let mut errors = Vec::new();
    for entry in entries {
        let mut cmd = Command::new(brew);
        if entry.kind == "tap" {
            cmd.arg("tap");
        } else {
            let mut check = Command::new(brew);
            check.args([
                "list",
                if entry.kind == "cask" {
                    "--cask"
                } else {
                    "--formula"
                },
                "--versions",
                &entry.name,
            ]);
            let installed = run_with_timeout(check, std::time::Duration::from_secs(60))
                .map(|o| o.status.success() && !o.stdout.is_empty())
                .unwrap_or(false);
            if installed && !reinstall {
                continue;
            }
            cmd.arg(if installed && reinstall {
                "reinstall"
            } else {
                "install"
            });
            cmd.arg(if entry.kind == "cask" {
                "--cask"
            } else {
                "--formula"
            });
        }
        cmd.arg(&entry.name);
        let outcome = run_streamed(
            cmd,
            std::time::Duration::from_secs(7200),
            window,
            "restore-log",
            "🍺 ",
            1,
        )
        .and_then(|o| require_success(&format!("brew {}", entry.name), &o));
        match outcome {
            Ok(()) => count += 1,
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(count)
    } else {
        Err(format!(
            "{count} Homebrew entries completed; {}",
            errors.join("; ")
        ))
    }
}
pub(super) fn install_extensions(
    code: &str,
    extensions: &[String],
    reinstall: bool,
) -> Result<usize, String> {
    let mut installed = 0;
    let mut errors = Vec::new();
    for chunk in extensions.chunks(6) {
        let mut handles = Vec::new();
        for ext in chunk {
            let code = code.to_string();
            let ext = ext.clone();
            handles.push(std::thread::spawn(move || {
                let mut cmd = Command::new(code);
                cmd.arg("--install-extension").arg(&ext);
                if reinstall {
                    cmd.arg("--force");
                }
                run_with_timeout(cmd, std::time::Duration::from_secs(900))
                    .and_then(|o| require_success(&ext, &o))
            }));
        }
        for h in handles {
            match h.join() {
                Ok(Ok(())) => installed += 1,
                Ok(Err(e)) => errors.push(e),
                Err(_) => errors.push("Extension worker failed".into()),
            }
        }
    }
    if errors.is_empty() {
        Ok(installed)
    } else {
        Err(format!(
            "{installed}/{} extensions installed; {}",
            extensions.len(),
            errors.join("; ")
        ))
    }
}
static ACTIVE_DISK_OPERATION: AtomicBool = AtomicBool::new(false);
pub(super) struct OperationGuard;
impl OperationGuard {
    pub fn acquire() -> Result<Self, String> {
        ACTIVE_DISK_OPERATION
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "Another backup, restore or verification is already running")?;
        BACKUP_CANCELLED.store(false, Ordering::SeqCst);
        VERIFY_CANCELLED.store(false, Ordering::SeqCst);
        Ok(Self)
    }
}
impl Drop for OperationGuard {
    fn drop(&mut self) {
        ACTIVE_DISK_OPERATION.store(false, Ordering::SeqCst);
    }
}
pub(super) fn quick_restore(
    backup: &Path,
    home: &Path,
    window: Option<&tauri::Window>,
) -> Result<RestoreResult, String> {
    let meta = load_backup_metadata(&backup.join("metadata.json"))?;
    let essentials = [
        ".ssh",
        ".gnupg",
        ".gitconfig",
        ".zshrc",
        ".zprofile",
        ".zsh_history",
        ".bashrc",
        ".bash_profile",
        ".bash_history",
        ".config/git",
        "Documents",
        "Desktop",
        "Pictures",
        ".config",
        "Library/LaunchAgents",
    ];
    let mut files = Vec::new();
    for item in &meta.items {
        let relative = item
            .path
            .strip_prefix("~/")
            .map(str::to_string)
            .or_else(|| {
                Path::new(&item.path)
                    .strip_prefix(home)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            });
        if relative.as_deref().is_some_and(|p| essentials.contains(&p)) {
            files.push(item.path.clone());
        }
    }
    let mut all = files.clone();
    for special in ["homebrew-packages", "vscode-extensions"] {
        if meta.items.iter().any(|i| i.path == special) {
            all.push(special.into());
        }
    }
    preflight_restore(backup, &meta, &all)?;
    let mut result = if files.is_empty() {
        RestoreResult {
            restored_count: 0,
            skipped_count: 0,
            error_count: 0,
            restored: vec![],
            skipped: vec![],
            errors: vec![],
        }
    } else {
        restore_selected(backup, &files, false, home, window)?
    };
    if let Some(item) = meta.items.iter().find(|i| i.path == "homebrew-packages") {
        let packages = brew_entries(&read_inventory(
            &backup.join(&item.archive),
            "homebrew_packages.txt",
        )?)?;
        let brews = [
            "git",
            "vim",
            "python",
            "node",
            "curl",
            "wget",
            "htop",
            "tree",
            "jq",
            "ripgrep",
            "fd",
            "bat",
            "fzf",
            "zsh-autosuggestions",
            "zsh-syntax-highlighting",
            "tmux",
        ];
        let casks = [
            "visual-studio-code",
            "iterm2",
            "google-chrome",
            "firefox",
            "1password",
            "rectangle",
            "alfred",
        ];
        let selected: Vec<_> = packages
            .into_iter()
            .filter(|p| {
                p.kind == "tap"
                    || (p.kind == "brew" && brews.contains(&p.name.as_str()))
                    || (p.kind == "cask" && casks.contains(&p.name.as_str()))
            })
            .collect();
        let r = if selected.is_empty() {
            Ok(0)
        } else {
            find_brew_path()
                .ok_or_else(|| "Homebrew not installed".into())
                .and_then(|brew| install_brew_entries(&brew, &selected, false, window))
        };
        match r {
            Ok(0) => result
                .skipped
                .push("homebrew-packages: no missing essentials".into()),
            Ok(n) => result
                .restored
                .push(format!("homebrew-packages: {n} essentials")),
            Err(e) => result.errors.push(e),
        }
    }
    if let Some(item) = meta.items.iter().find(|i| i.path == "vscode-extensions") {
        match restore_vscode_extensions(backup, &item.archive, false) {
            Ok(n) => result.restored.push(format!("vscode-extensions: {n}")),
            Err(e) => result.errors.push(e),
        }
    }
    result.restored_count = result.restored.len();
    result.skipped_count = result.skipped.len();
    result.error_count = result.errors.len();
    Ok(result)
}

pub(super) fn ensure_operation_idle() -> Result<(), String> {
    if ACTIVE_DISK_OPERATION.load(Ordering::SeqCst) {
        Err("Another operation is still running".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn archive_reader_stops_between_reads_on_cancel() {
        let _guard=OperationGuard::acquire().unwrap();
        let mut reader=ArchiveInput {reader:Box::new(io::Cursor::new(vec![1u8;1024])),child:None};
        let mut buffer=[0;16];assert_eq!(reader.read(&mut buffer).unwrap(),16);
        cancel_operation().unwrap();
        let error=reader.read(&mut buffer).unwrap_err();
        assert_ne!(error.kind(),io::ErrorKind::Interrupted);
        assert!(error.to_string().contains("abgebrochen"));
        assert!(OperationGuard::acquire().is_err());
        assert!(reset_operation_state().is_err());
        BACKUP_CANCELLED.store(false,Ordering::SeqCst);VERIFY_CANCELLED.store(false,Ordering::SeqCst);
    }
}
