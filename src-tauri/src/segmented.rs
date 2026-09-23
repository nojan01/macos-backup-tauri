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
#[derive(Debug, Serialize, Deserialize, Clone)]
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
enum DecodedPayload {
    Disk(fs::File),
    Ram(std::io::Cursor<Vec<u8>>),
}
impl Read for DecodedPayload {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Disk(f) => f.read(buf),
            Self::Ram(c) => c.read(buf),
        }
    }
}
struct Reader {
    file: fs::File,
    path: PathBuf,
    index: Index,
    next: usize,
    payload: Option<DecodedPayload>,
    stage: Option<crate::backup::ReadbackDir>,
}
impl Reader {
    fn open(path: &Path) -> io::Result<Self> {
        if !crate::ram_parts::ready() {
            crate::backup::require_free_space(&std::env::temp_dir(), WORK_BYTES).map_err(error)?;
        }
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
            if part.raw <= crate::ram_parts::PART_BYTES && crate::ram_parts::ready() {
                let data = decode_part_ram(&mut self.file, &self.path, part)?;
                self.payload = Some(DecodedPayload::Ram(std::io::Cursor::new(data)));
            } else {
                crate::backup::require_free_space(&std::env::temp_dir(), WORK_BYTES)
                    .map_err(error)?;
                let stage = crate::backup::ReadbackDir(PrivateDir::temp().map_err(error)?);
                let payload = decode_part(&mut self.file, &self.path, part, &stage.0 .0)?;
                self.payload = Some(DecodedPayload::Disk(fs::File::open(payload)?));
                self.stage = Some(stage);
            }
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

fn decode_part_ram(file: &mut fs::File, path: &Path, part: &Part) -> io::Result<Vec<u8>> {
    cancelled()?;
    if part.raw > crate::ram_parts::PART_BYTES
        || part.compressed > crate::ram_parts::PART_BYTES + 16 * 1024 * 1024
    {
        return Err(error("RAM-Teilarchiv zu groß"));
    }
    file.seek(SeekFrom::Start(part.offset))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(part.compressed as usize)
        .map_err(error)?;
    let mut digest = Sha256::new();
    let mut remaining = part.compressed;
    let mut buf = vec![0u8; 1024 * 1024];
    let limiter = crate::throttle::limiter_for(path);
    while remaining > 0 {
        cancelled()?;
        let n = remaining.min(buf.len() as u64) as usize;
        if let Some(limiter) = &limiter {
            limiter.acquire(n as u64)?;
        }
        file.read_exact(&mut buf[..n])?;
        digest.update(&buf[..n]);
        bytes.extend_from_slice(&buf[..n]);
        remaining -= n as u64;
    }
    if hex(&digest.finalize()) != part.compressed_sha256 {
        return Err(error(
            "Teilarchiv beschädigt: komprimierte Prüfsumme stimmt nicht",
        ));
    }
    crate::ram_parts::decode(bytes, part.raw, &part.raw_sha256)
}

struct DiskWriter {
    file: fs::File,
    path: PathBuf,
    index: Index,
    limit: u64,
    stage: PrivateDir,
    payload: fs::File,
    raw: u64,
    hash: Sha256,
}
impl DiskWriter {
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
impl Write for DiskWriter {
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
struct RamWriter {
    file: fs::File,
    path: PathBuf,
    index: Index,
    limit: u64,
    payload: Vec<u8>,
    hash: Sha256,
    disk: Option<DiskWriter>,
}
impl RamWriter {
    fn new(path: &Path, limit: u64) -> io::Result<Self> {
        let payload = Vec::new();
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
            payload,
            hash: Sha256::new(),
            disk: None,
        })
    }
    fn spill_to_disk(&mut self) -> io::Result<()> {
        if self.disk.is_some() {
            return Ok(());
        }
        crate::backup::require_free_space(&std::env::temp_dir(), WORK_BYTES).map_err(error)?;
        let stage = PrivateDir::temp().map_err(error)?;
        let mut payload = fs::File::create(stage.0.join("payload"))?;
        payload.write_all(&self.payload)?;
        let raw = self.payload.len() as u64;
        self.payload = Vec::new();
        let disk = DiskWriter {
            file: self.file.try_clone()?,
            path: self.path.clone(),
            index: self.index.clone(),
            limit: self.limit,
            stage,
            payload,
            raw,
            hash: self.hash.clone(),
        };
        self.disk = Some(disk);
        crate::work_progress::detail(
            "Wenig freier RAM: Teilarchive werden auf der internen SSD verarbeitet".into(),
            true,
        );
        Ok(())
    }
    fn part(&mut self) -> io::Result<()> {
        if let Some(disk) = self.disk.as_mut() {
            return disk.part();
        }
        if self.payload.is_empty() {
            return Ok(());
        }
        if !crate::ram_parts::ready() {
            self.spill_to_disk()?;
            return self.disk.as_mut().unwrap().part();
        }
        cancelled()?;
        let number = self.index.parts.len() + 1;
        let _phase = crate::work_progress::Phase::enter(&format!(
            "Teilarchiv {number} im RAM komprimieren und zurücklesen"
        ));
        let size = self.payload.len() as u64;
        let compressed = match crate::ram_parts::encode(&self.payload) {
            Ok(bytes) => bytes,
            Err(e) if e.to_string() != "Backup abgebrochen" => {
                self.spill_to_disk()?;
                return self.disk.as_mut().unwrap().part();
            }
            Err(e) => return Err(e),
        };
        self.payload = Vec::new();
        let expected_hash = hex(&std::mem::take(&mut self.hash).finalize());
        let compressed_size = compressed.len() as u64;
        if compressed_size > MAX_COMPRESSED {
            return Err(error("Teilarchiv überschreitet Größenbegrenzung"));
        }
        crate::backup::require_free_space(
            self.path
                .parent()
                .ok_or_else(|| error("Archiv ohne Zielordner"))?,
            compressed_size + 2 * PART_BYTES,
        )
        .map_err(error)?;
        let part = Part {
            offset: self.file.stream_position()?,
            compressed: compressed_size,
            raw: size,
            compressed_sha256: hex(&Sha256::digest(&compressed)),
            raw_sha256: expected_hash,
        };
        let limiter = crate::throttle::limiter_for(&self.path);
        for chunk in compressed.chunks(1024 * 1024) {
            cancelled()?;
            if let Some(limiter) = &limiter {
                limiter.acquire(chunk.len() as u64)?;
            }
            self.file.write_all(chunk)?;
        }
        self.file.flush()?;
        drop(compressed);
        if crate::ram_parts::ready() {
            let checked = decode_part_ram(&mut self.file, &self.path, &part)?;
            drop(checked);
        } else {
            crate::backup::require_free_space(&std::env::temp_dir(), WORK_BYTES).map_err(error)?;
            let check = crate::backup::ReadbackDir(PrivateDir::temp().map_err(error)?);
            decode_part(&mut self.file, &self.path, &part, &check.0 .0)?;
        }
        self.file
            .seek(SeekFrom::Start(part.offset + part.compressed))?;
        self.index.raw_bytes += part.raw;
        self.index.parts.push(part);
        crate::work_progress::detail(
            format!("Teilarchiv {number} im RAM geprüft; Prüfkopie freigegeben"),
            true,
        );
        Ok(())
    }
    fn finish(mut self) -> io::Result<()> {
        self.part()?;
        if let Some(disk) = self.disk.take() {
            return disk.finish();
        }
        finish_index(&mut self.file, &self.path, &self.index)
    }
}
impl Write for RamWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        cancelled()?;
        if self.disk.as_ref().is_some_and(|disk| disk.raw == 0) && crate::ram_parts::ready() {
            let disk = self.disk.take().unwrap();
            self.index = disk.index.clone();
            self.limit = crate::ram_parts::PART_BYTES.min(disk.limit);
            self.hash = Sha256::new();
            self.payload = Vec::new();
            crate::work_progress::detail(
                "Genug freier RAM: weitere Teilarchive im Arbeitsspeicher".into(),
                true,
            );
        }
        if let Some(disk) = self.disk.as_mut() {
            return disk.write(buf);
        }
        if !crate::ram_parts::ready()
            || (self.payload.is_empty()
                && self.payload.try_reserve_exact(self.limit as usize).is_err())
        {
            self.spill_to_disk()?;
            return self.disk.as_mut().unwrap().write(buf);
        }
        let n = (self.limit - self.payload.len() as u64).min(buf.len() as u64) as usize;
        self.payload.extend_from_slice(&buf[..n]);
        self.hash.update(&buf[..n]);
        if self.payload.len() as u64 == self.limit {
            self.part()?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        if let Some(disk) = self.disk.as_mut() {
            disk.flush()
        } else {
            Ok(())
        }
    }
}
fn finish_index(file: &mut fs::File, path: &Path, index: &Index) -> io::Result<()> {
    let data = serde_json::to_vec(index).map_err(error)?;
    if data.len() as u64 > MAX_INDEX || index.parts.is_empty() {
        return Err(error("Teilarchiv-Index zu groß oder leer"));
    }
    let limiter = crate::throttle::limiter_for(path);
    for chunk in data.chunks(1024 * 1024) {
        cancelled()?;
        if let Some(limiter) = &limiter {
            limiter.acquire(chunk.len() as u64)?;
        }
        file.write_all(chunk)?;
    }
    file.write_all(&Sha256::digest(&data))?;
    file.write_all(&(data.len() as u64).to_le_bytes())?;
    file.write_all(FOOTER)?;
    file.flush()
}
enum Writer {
    Disk(DiskWriter),
    Ram(RamWriter),
}
impl Writer {
    fn new(path: &Path, limit: u64) -> io::Result<Self> {
        if limit <= crate::ram_parts::PART_BYTES && crate::ram_parts::ready() {
            Ok(Self::Ram(RamWriter::new(path, limit)?))
        } else {
            Ok(Self::Disk(DiskWriter::new(path, limit)?))
        }
    }
    fn finish(self) -> io::Result<()> {
        match self {
            Self::Disk(w) => w.finish(),
            Self::Ram(w) => w.finish(),
        }
    }
}
impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Self::Disk(w) = self {
            if w.raw == 0 && crate::ram_parts::ready() {
                let ram = RamWriter {
                    file: w.file.try_clone()?,
                    path: w.path.clone(),
                    index: w.index.clone(),
                    limit: crate::ram_parts::PART_BYTES.min(w.limit),
                    payload: Vec::new(),
                    hash: Sha256::new(),
                    disk: None,
                };
                *self = Self::Ram(ram);
                crate::work_progress::detail(
                    "Genug freier RAM: weitere Teilarchive im Arbeitsspeicher".into(),
                    true,
                );
            }
        }
        match self {
            Self::Disk(w) => w.write(buf),
            Self::Ram(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Disk(w) => w.flush(),
            Self::Ram(w) => w.flush(),
        }
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
    let _phase = crate::work_progress::Phase::enter(&format!(
        "Teilarchive erstellen: {}",
        source.file_name().unwrap_or_default().to_string_lossy()
    ));
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
            create_with_limit(
                source,
                &temporary,
                root,
                expected,
                if crate::ram_parts::ready() {
                    crate::ram_parts::PART_BYTES
                } else {
                    PART_BYTES
                },
            )?;
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
            if let Some(stage) = reader.stage.as_ref() {
                let current = stage.0 .0.clone();
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
        }
        if let Some(previous) = previous {
            assert!(!previous.exists());
        }
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
    #[ignore = "end-to-end 129 MiB RAM part-boundary test"]
    fn real_ram_backup_crosses_128_mib_boundary_and_verifies() {
        let dir = fixture();
        let source = dir.0.join("large.bin");
        fs::File::create(&source)
            .unwrap()
            .set_len(crate::ram_parts::PART_BYTES + 1024)
            .unwrap();
        let expected = crate::backup::compute_snapshot(&source).unwrap();
        let archive = dir.0.join("large.aarset");
        crate::ram_parts::force_ready_for_test(Some(true));
        create_with_limit(
            &source,
            &archive,
            "large.bin",
            &expected,
            crate::ram_parts::PART_BYTES,
        )
        .unwrap();
        let mut f = fs::File::open(&archive).unwrap();
        let idx = index(&mut f).unwrap();
        assert!(idx.parts.len() >= 2);
        crate::backup::verify_archive_source(&archive, "large.bin", &expected).unwrap();
        crate::ram_parts::force_ready_for_test(None);
    }
    #[test]
    fn low_memory_spills_a_partial_ram_part_to_disk_and_preserves_bytes() {
        let dir = fixture();
        let archive = dir.0.join("spill.aarset");
        crate::ram_parts::force_ready_for_test(Some(true));
        let mut writer = RamWriter::new(&archive, 4096).unwrap();
        writer.write_all(b"first half").unwrap();
        crate::ram_parts::force_ready_for_test(Some(false));
        writer.write_all(b" plus second half").unwrap();
        writer.finish().unwrap();
        crate::ram_parts::force_ready_for_test(Some(true));
        let mut restored = Vec::new();
        Reader::open(&archive)
            .unwrap()
            .read_to_end(&mut restored)
            .unwrap();
        crate::ram_parts::force_ready_for_test(None);
        assert_eq!(restored, b"first half plus second half");
    }
    #[test]
    fn switches_disk_to_ram_and_back_without_changing_container_format() {
        let dir = fixture();
        let archive = dir.0.join("switches.aarset");
        crate::ram_parts::force_ready_for_test(Some(false));
        let mut writer = Writer::new(&archive, 4096).unwrap();
        writer.write_all(&vec![1; 4096]).unwrap();
        crate::ram_parts::force_ready_for_test(Some(true));
        writer.write_all(&vec![2; 10]).unwrap();
        crate::ram_parts::force_ready_for_test(Some(false));
        writer.write_all(&vec![2; 4086]).unwrap();
        crate::ram_parts::force_ready_for_test(Some(true));
        writer.write_all(&vec![3; 10]).unwrap();
        writer.finish().unwrap();
        let mut restored = Vec::new();
        Reader::open(&archive)
            .unwrap()
            .read_to_end(&mut restored)
            .unwrap();
        crate::ram_parts::force_ready_for_test(None);
        assert_eq!(
            restored,
            [&vec![1; 4096][..], &vec![2; 4096][..], &vec![3; 10][..]].concat()
        );
    }
    #[test]
    fn old_disk_parts_are_read_from_ram_when_available() {
        let dir = fixture();
        let archive = dir.0.join("old.aarset");
        crate::ram_parts::force_ready_for_test(Some(false));
        let mut writer = DiskWriter::new(&archive, 4096).unwrap();
        writer.write_all(b"existing disk-backed part").unwrap();
        writer.finish().unwrap();
        crate::ram_parts::force_ready_for_test(Some(true));
        let mut reader = Reader::open(&archive).unwrap();
        let mut restored = Vec::new();
        reader.read_to_end(&mut restored).unwrap();
        assert!(reader.stage.is_none());
        crate::ram_parts::force_ready_for_test(None);
        assert_eq!(restored, b"existing disk-backed part");
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
