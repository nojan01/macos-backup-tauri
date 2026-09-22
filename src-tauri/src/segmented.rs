//! Versioned, self-contained set of independently compressed AppleArchive parts.
//! Parts split the native raw stream, including inside a single large DAT blob.
//! Only owned temporary payloads are removed; published backup parts are retained.
use super::*;
use std::io::{self, Seek, SeekFrom, Write};

pub(crate) const PART_BYTES: u64 = 1024 * 1024 * 1024;
pub(crate) const WORK_BYTES: u64 = 5 * PART_BYTES;
const HEADER: &[u8; 8] = b"MBS2AA01";
const FOOTER: &[u8; 8] = b"MBS2END1";
const MAX_INDEX: u64 = 64 * 1024 * 1024;
const MAX_COMPRESSED: u64 = PART_BYTES + 16 * 1024 * 1024;
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Part {
    offset: u64,
    compressed: u64,
    raw: u64,
    compressed_sha256: String,
    raw_sha256: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    raw_bytes: u64,
    parts: Vec<Part>,
}
fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn cancelled() -> io::Result<()> {
    if BACKUP_CANCELLED.load(Ordering::SeqCst) || VERIFY_CANCELLED.load(Ordering::SeqCst) {
        Err(error("Backup abgebrochen"))
    } else {
        Ok(())
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(super) fn is_segmented(path: &Path) -> Result<bool, String> {
    let mut magic = [0; 8];
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let n = file.read(&mut magic).map_err(|e| e.to_string())?;
    Ok(n == magic.len() && &magic == HEADER)
}
pub(super) fn open(path: &Path) -> Result<Box<dyn Read + Send>, String> {
    if is_segmented(path)? {
        Ok(Box::new(Reader::open(path).map_err(|e| e.to_string())?))
    } else {
        Ok(Box::new(
            crate::throttle::open_throttled(path).map_err(|e| e.to_string())?,
        ))
    }
}
fn index(file: &mut fs::File) -> io::Result<Index> {
    let len = file.metadata()?.len();
    if len < 8 + 48 {
        return Err(error("Unvollständiger Teilarchiv-Container"));
    }
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    if &magic != HEADER {
        return Err(error("Ungültiger Teilarchiv-Kopf"));
    }
    file.seek(SeekFrom::End(-48))?;
    let mut digest = [0; 32];
    file.read_exact(&mut digest)?;
    let mut length = [0; 8];
    file.read_exact(&mut length)?;
    file.read_exact(&mut magic)?;
    let size = u64::from_le_bytes(length);
    if &magic != FOOTER || size == 0 || size > MAX_INDEX || size > len - 56 {
        return Err(error("Ungültiger oder fehlender Teilarchiv-Index"));
    }
    let start = len - 48 - size;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes)?;
    if Sha256::digest(&bytes)[..] != digest {
        return Err(error("Teilarchiv-Index: Prüfsumme stimmt nicht"));
    }
    let idx: Index = serde_json::from_slice(&bytes).map_err(error)?;
    if idx.version != 1 || idx.parts.is_empty() {
        return Err(error("Nicht unterstützter Teilarchiv-Index"));
    }
    let mut offset = 8u64;
    let mut raw = 0u64;
    for p in &idx.parts {
        if p.offset != offset
            || p.raw == 0
            || p.raw > PART_BYTES
            || p.compressed == 0
            || p.compressed > MAX_COMPRESSED
            || !valid_hash(&p.raw_sha256)
            || !valid_hash(&p.compressed_sha256)
        {
            return Err(error("Ungültige Reihenfolge oder Größe der Teilarchive"));
        }
        offset = offset
            .checked_add(p.compressed)
            .ok_or_else(|| error("Teilarchiv-Größenüberlauf"))?;
        raw = raw
            .checked_add(p.raw)
            .ok_or_else(|| error("Teilarchiv-Größenüberlauf"))?;
    }
    if offset != start || raw != idx.raw_bytes {
        return Err(error("Fehlende oder zusätzliche Teilarchiv-Daten"));
    }
    Ok(idx)
}

/// Keeps a single decoded part alive. The previous part is dropped before loading
/// the next, so even a multi-terabyte file has a bounded verification footprint.
struct Reader {
    file: fs::File,
    path: PathBuf,
    index: Index,
    next: usize,
    payload: Option<fs::File>,
    stage: Option<crate::backup::ReadbackDir>,
}
impl Reader {
    fn open(path: &Path) -> io::Result<Self> {
        crate::backup::require_free_space(&std::env::temp_dir(), WORK_BYTES).map_err(error)?;
        let mut file = fs::File::open(path)?;
        let index = index(&mut file)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            index,
            next: 0,
            payload: None,
            stage: None,
        })
    }
}
impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            cancelled()?;
            if let Some(payload) = self.payload.as_mut() {
                let n = payload.read(buf)?;
                if n != 0 {
                    return Ok(n);
                }
            }
            self.payload = None;
            self.stage = None;
            let Some(part) = self.index.parts.get(self.next) else {
                return Ok(0);
            };
            let stage = crate::backup::ReadbackDir(PrivateDir::temp().map_err(error)?);
            let payload = decode_part(&mut self.file, &self.path, part, &stage.0 .0)?;
            self.payload = Some(fs::File::open(payload)?);
            self.stage = Some(stage);
            self.next += 1;
            crate::work_progress::detail(
                format!(
                    "Teilarchiv {}/{} zurückgelesen und geprüft",
                    self.next,
                    self.index.parts.len()
                ),
                true,
            );
        }
    }
}
fn decode_part(file: &mut fs::File, path: &Path, part: &Part, stage: &Path) -> io::Result<PathBuf> {
    cancelled()?;
    file.seek(SeekFrom::Start(part.offset))?;
    let compressed = stage.join("part.aar");
    let mut output = fs::File::create(&compressed)?;
    let mut remaining = part.compressed;
    let mut digest = Sha256::new();
    let mut buf = vec![0; 1024 * 1024];
    let limiter = crate::throttle::limiter_for(path);
    while remaining > 0 {
        cancelled()?;
        let n = remaining.min(buf.len() as u64) as usize;
        if let Some(limiter) = &limiter {
            limiter.acquire(n as u64)?;
        }
        file.read_exact(&mut buf[..n])?;
        output.write_all(&buf[..n])?;
        digest.update(&buf[..n]);
        remaining -= n as u64;
    }
    drop(output);
    if hex(&digest.finalize()) != part.compressed_sha256 {
        return Err(error(
            "Teilarchiv beschädigt: komprimierte Prüfsumme stimmt nicht",
        ));
    }
    let mut magic = [0; 4];
    fs::File::open(&compressed)?.read_exact(&mut magic)?;
    if &magic != b"pbze" {
        return Err(error("Teilarchive müssen native LZFSE-Archive sein"));
    }
    let entries = crate::apple_archive::inspect(&compressed).map_err(error)?;
    if entries.len() != 1
        || !entries.get(Path::new("payload")).is_some_and(|e| {
            e.kind == "F"
                && e.size == part.raw
                && e.hardlink.is_none()
                && e.xattr_size <= 65536
                && e.acl_size <= 65536
        })
    {
        return Err(error("Ungültiger Inhalt eines Teilarchivs"));
    }
    let decoded = stage.join("decoded");
    fs::create_dir(&decoded)?;
    crate::apple_archive::extract(&compressed, &decoded, Some(std::ffi::OsStr::new("payload")))
        .map_err(error)?;
    fs::remove_file(compressed)?;
    let payload = decoded.join("payload");
    if fs::symlink_metadata(&payload)?.len() != part.raw
        || crate::hash_file(&payload).map_err(error)? != part.raw_sha256
    {
        return Err(error(
            "Teilarchiv beschädigt: entpackte Prüfsumme stimmt nicht",
        ));
    }
    Ok(payload)
}

struct Writer {
    file: fs::File,
    path: PathBuf,
    index: Index,
    limit: u64,
    stage: PrivateDir,
    payload: fs::File,
    raw: u64,
    hash: Sha256,
}
impl Writer {
    fn new(path: &Path, limit: u64) -> io::Result<Self> {
        if limit == 0 || limit > PART_BYTES {
            return Err(error("Ungültige Teilarchiv-Größe"));
        }
        crate::backup::require_free_space(&std::env::temp_dir(), WORK_BYTES).map_err(error)?;
        let stage = PrivateDir::temp().map_err(error)?;
        let payload = fs::File::create(stage.0.join("payload"))?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(HEADER)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            index: Index {
                version: 1,
                raw_bytes: 0,
                parts: Vec::new(),
            },
            limit,
            stage,
            payload,
            raw: 0,
            hash: Sha256::new(),
        })
    }
    fn part(&mut self) -> io::Result<()> {
        if self.raw == 0 {
            return Ok(());
        }
        cancelled()?;
        self.payload.flush()?;
        crate::backup::require_free_space(&std::env::temp_dir(), WORK_BYTES - self.raw)
            .map_err(error)?;
        let number = self.index.parts.len() + 1;
        let _phase = crate::work_progress::Phase::enter(&format!(
            "Teilarchiv {number} komprimieren und zurücklesen"
        ));
        let compressed = self.stage.0.join("write.aar");
        crate::apple_archive::create(&self.stage.0.join("payload"), &compressed).map_err(error)?;
        let compressed_size = fs::metadata(&compressed)?.len();
        if compressed_size > MAX_COMPRESSED {
            return Err(error("Teilarchiv überschreitet die Größenbegrenzung"));
        }
        crate::backup::require_free_space(
            self.path
                .parent()
                .ok_or_else(|| error("Archiv ohne Zielordner"))?,
            compressed_size + 2 * PART_BYTES,
        )
        .map_err(error)?;
        let mut part = Part {
            offset: self.file.stream_position()?,
            compressed: compressed_size,
            raw: self.raw,
            compressed_sha256: String::new(),
            raw_sha256: hex(&std::mem::take(&mut self.hash).finalize()),
        };
        let mut input = fs::File::open(&compressed)?;
        let mut digest = Sha256::new();
        let mut buf = vec![0; 1024 * 1024];
        let limiter = crate::throttle::limiter_for(&self.path);
        loop {
            cancelled()?;
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if let Some(limiter) = &limiter {
                limiter.acquire(n as u64)?;
            }
            self.file.write_all(&buf[..n])?;
            digest.update(&buf[..n]);
        }
        part.compressed_sha256 = hex(&digest.finalize());
        self.file.flush()?;
        // Use the bytes read back from the destination, not the source spool.
        drop(input);
        fs::remove_file(&compressed)?;
        self.payload.set_len(0)?;
        self.payload.rewind()?;
        let check = crate::backup::ReadbackDir(PrivateDir::temp().map_err(error)?);
        decode_part(&mut self.file, &self.path, &part, &check.0 .0)?;
        drop(check);
        self.file
            .seek(SeekFrom::Start(part.offset + part.compressed))?;
        self.index.raw_bytes += part.raw;
        self.index.parts.push(part);
        self.raw = 0;
        crate::work_progress::detail(
            format!("Teilarchiv {number} geprüft; temporäre Prüfkopie entfernt"),
            true,
        );
        Ok(())
    }
    fn finish(mut self) -> io::Result<()> {
        self.part()?;
        let data = serde_json::to_vec(&self.index).map_err(error)?;
        if data.len() as u64 > MAX_INDEX || self.index.parts.is_empty() {
            return Err(error("Teilarchiv-Index zu groß oder leer"));
        }
        let limiter = crate::throttle::limiter_for(&self.path);
        for chunk in data.chunks(1024 * 1024) {
            cancelled()?;
            if let Some(limiter) = &limiter {
                limiter.acquire(chunk.len() as u64)?;
            }
            self.file.write_all(chunk)?;
        }
        self.file.write_all(&Sha256::digest(&data))?;
        self.file.write_all(&(data.len() as u64).to_le_bytes())?;
        self.file.write_all(FOOTER)?;
        self.file.flush()
    }
}
impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        cancelled()?;
        let n = (self.limit - self.raw).min(buf.len() as u64) as usize;
        self.payload.write_all(&buf[..n])?;
        self.hash.update(&buf[..n]);
        self.raw += n as u64;
        if self.raw == self.limit {
            self.part()?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.payload.flush()
    }
}
struct Tee<R> {
    source: R,
    writer: Writer,
}
impl<R: Read> Read for Tee<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.source.read(buf)?;
        self.writer.write_all(&buf[..n])?;
        Ok(n)
    }
}
pub(super) fn create(
    source: &Path,
    target: &Path,
    root: &str,
    expected: &[crate::backup::ManifestEntry],
) -> Result<(), String> {
    // Keep the established screen-lock pause/retry behavior. Every attempt owns
    // a separate unpublished container, so a retry cannot append duplicate parts.
    if fs::symlink_metadata(target).is_ok() {
        return Err("Teilarchiv-Ziel existiert bereits".into());
    }
    let mut last_error = None;
    let result = crate::protected_access::retry(|| {
        let attempt = (|| -> Result<(), String> {
            let stage =
                PrivateDir::new(target.parent().ok_or("Archiv ohne Zielordner")?, ".parts")?;
            let temporary = stage.0.join("container");
            create_with_limit(source, &temporary, root, expected, PART_BYTES)?;
            fs::rename(&temporary, target).map_err(|e| e.to_string())
        })();
        attempt.map_err(|message| {
            let denied = message.contains("Operation not permitted")
                || message.contains("Permission denied");
            last_error = Some(message.clone());
            if denied {
                io::Error::new(io::ErrorKind::PermissionDenied, message)
            } else {
                error(message)
            }
        })
    });
    match result {
        Ok(()) => Ok(()),
        Err(crate::protected_access::AccessError::Cancelled) => Err("Backup abgebrochen".into()),
        Err(crate::protected_access::AccessError::Io(e)) => {
            Err(last_error.unwrap_or_else(|| e.to_string()))
        }
    }
}

fn create_with_limit(
    source: &Path,
    target: &Path,
    root: &str,
    expected: &[crate::backup::ManifestEntry],
    limit: u64,
) -> Result<(), String> {
    let raw = crate::apple_archive::RawSource::open(source)?;
    let mut tee = Tee {
        source: raw,
        writer: Writer::new(target, limit).map_err(|e| e.to_string())?,
    };
    let result = crate::backup::verify_raw_stream(&mut tee, root, expected).map_err(|e| {
        format!(
            "Quelle: {}; Archivziel: {}: {e}",
            source.display(),
            target.display()
        )
    });
    crate::work_progress::report_result(result)?;
    tee.writer.finish().map_err(|e| e.to_string())?;
    // Validate the just-written final index/footer as well as all decoded parts.
    index(&mut fs::File::open(target).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt};
    fn fixture() -> PrivateDir {
        BACKUP_CANCELLED.store(false, Ordering::SeqCst);
        VERIFY_CANCELLED.store(false, Ordering::SeqCst);
        PrivateDir::temp().unwrap()
    }
    fn parts_archive(dir: &PrivateDir) -> (PathBuf, PathBuf, Vec<crate::backup::ManifestEntry>) {
        let source = dir.0.join("source");
        fs::create_dir(&source).unwrap();
        let bytes: Vec<u8> = (0..20537u32)
            .map(|n| (n.wrapping_mul(71) % 251) as u8)
            .collect();
        fs::write(source.join("large"), &bytes).unwrap();
        xattr::set(
            source.join("large"),
            "com.apple.ResourceFork",
            &vec![43; 12293],
        )
        .unwrap();
        xattr::set(
            source.join("large"),
            "com.example.parts",
            b"binary\0metadata\xff",
        )
        .unwrap();
        fs::hard_link(source.join("large"), source.join("hardlink")).unwrap();
        symlink("large", source.join("link")).unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        let status = Command::new("/bin/chmod")
            .args(["+a", "everyone deny delete"])
            .arg(source.join("large"))
            .status()
            .unwrap();
        assert!(status.success());
        let expected = crate::backup::compute_snapshot(&source).unwrap();
        let archive = dir.0.join("source.aarset");
        create_with_limit(&source, &archive, "source", &expected, 4096).unwrap();
        (source, archive, expected)
    }
    #[test]
    fn split_single_file_resource_fork_and_hardlinks_restore_with_bounded_workspace() {
        let dir = fixture();
        let (source, archive, expected) = parts_archive(&dir);
        let mut reader = Reader::open(&archive).unwrap();
        assert!(reader.index.parts.len() > 10);
        assert!(reader.index.parts.iter().all(|p| p.raw <= 4096));
        let mut buffer = [0; 257];
        let mut previous: Option<PathBuf> = None;
        while reader.read(&mut buffer).unwrap() != 0 {
            let current = reader.stage.as_ref().unwrap().0 .0.clone();
            if previous.as_ref().is_some_and(|p| p != &current) {
                assert!(!previous.as_ref().unwrap().exists());
            }
            let size: u64 = WalkDir::new(&current)
                .into_iter()
                .map(|e| e.unwrap())
                .filter(|e| e.file_type().is_file())
                .map(|e| e.metadata().unwrap().len())
                .sum();
            assert!(size <= 4096, "only one decoded part is retained: {size}");
            previous = Some(current);
        }
        assert!(!previous.unwrap().exists());
        crate::backup::verify_archive_source(&archive, "source", &expected).unwrap();
        let output = PrivateDir::temp().unwrap();
        crate::apple_archive::extract(&archive, &output.0, Some(std::ffi::OsStr::new("source")))
            .unwrap();
        let restored = output.0.join("source");
        assert_eq!(
            fs::read(source.join("large")).unwrap(),
            fs::read(restored.join("large")).unwrap()
        );
        assert_eq!(
            fs::read_link(restored.join("link")).unwrap(),
            PathBuf::from("large")
        );
        assert_eq!(
            fs::metadata(restored.join("large")).unwrap().ino(),
            fs::metadata(restored.join("hardlink")).unwrap().ino()
        );
        let actual = crate::backup::compute_snapshot(&restored).unwrap();
        for (a, b) in actual.iter().zip(&expected) {
            let mut a = serde_json::to_value(a).unwrap();
            let mut b = serde_json::to_value(b).unwrap();
            for key in ["dev", "ino", "c", "cn", "uid", "gid"] {
                a.as_object_mut().unwrap().remove(key);
                b.as_object_mut().unwrap().remove(key);
            }
            a["xattrs"]
                .as_object_mut()
                .unwrap()
                .remove("com.apple.provenance");
            b["xattrs"]
                .as_object_mut()
                .unwrap()
                .remove("com.apple.provenance");
            assert_eq!(a, b);
        }
    }
    fn replace_index(path: &Path, idx: &Index) {
        let mut f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let old = index(&mut f).unwrap();
        let end = old.parts.last().unwrap();
        let pos = end.offset + end.compressed;
        f.set_len(pos).unwrap();
        f.seek(SeekFrom::Start(pos)).unwrap();
        let bytes = serde_json::to_vec(idx).unwrap();
        f.write_all(&bytes).unwrap();
        f.write_all(&Sha256::digest(&bytes)).unwrap();
        f.write_all(&(bytes.len() as u64).to_le_bytes()).unwrap();
        f.write_all(FOOTER).unwrap();
    }
    #[test]
    fn missing_reordered_corrupt_and_truncated_parts_fail_before_restore_publication() {
        let dir = fixture();
        let (_, archive, expected) = parts_archive(&dir);
        let original = fs::read(&archive).unwrap();
        for variant in 0..6 {
            fs::write(&archive, &original).unwrap();
            if variant < 4 {
                let mut f = fs::File::open(&archive).unwrap();
                let mut idx = index(&mut f).unwrap();
                match variant {
                    0 => {
                        idx.parts.swap(0, 1);
                    }
                    1 => {
                        idx.parts.remove(1);
                    }
                    2 => {
                        idx.parts[0].raw_sha256 = "0".repeat(64);
                    }
                    _ => {
                        idx.parts[0].raw = PART_BYTES + 1;
                    }
                }
                replace_index(&archive, &idx);
            } else if variant == 4 {
                let mut damaged = original.clone();
                damaged[24] ^= 0x55;
                fs::write(&archive, damaged).unwrap();
            } else {
                fs::write(&archive, &original[..original.len() - 1]).unwrap();
            }
            assert!(
                crate::backup::verify_archive_source(&archive, "source", &expected).is_err(),
                "variant {variant}"
            );
            let output = PrivateDir::temp().unwrap();
            assert!(
                crate::apple_archive::extract(&archive, &output.0, None).is_err(),
                "variant {variant}"
            );
            assert!(fs::read_dir(&output.0).unwrap().next().is_none());
        }
    }
    #[test]
    fn a_native_header_may_cross_part_boundaries() {
        let dir = fixture();
        let source = dir.0.join("empty-file");
        fs::write(&source, []).unwrap();
        let expected = crate::backup::compute_snapshot(&source).unwrap();
        let archive = dir.0.join("tiny.aarset");
        create_with_limit(&source, &archive, "empty-file", &expected, 29).unwrap();
        crate::backup::verify_archive_source(&archive, "empty-file", &expected).unwrap();
        let output = PrivateDir::temp().unwrap();
        crate::apple_archive::extract(&archive, &output.0, None).unwrap();
        assert_eq!(fs::read(output.0.join("empty-file")).unwrap(), b"");
    }
}
