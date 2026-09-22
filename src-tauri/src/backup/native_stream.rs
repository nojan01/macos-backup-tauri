//! Validate the raw native AppleArchive stream without materializing file data.
//! Apple's SDK parses headers; DAT and XAT values are hashed incrementally.
use super::*;
use std::collections::BTreeSet;
type Handle = *mut libc::c_void;
#[repr(C)]
#[derive(Clone, Copy)]
struct Key(u32);
fn key(s: &[u8; 3]) -> Key {
    Key(u32::from_le_bytes([s[0], s[1], s[2], 0]))
}
#[link(name = "AppleArchive")]
unsafe extern "C" {
    fn AAHeaderCreateWithEncodedData(size: usize, data: *const u8) -> Handle;
    fn AAHeaderDestroy(header: Handle);
    fn AAHeaderGetFieldCount(header: Handle) -> u32;
    fn AAHeaderGetKeyIndex(header: Handle, key: Key) -> i32;
    fn AAHeaderGetFieldType(header: Handle, index: u32) -> i32;
    fn AAHeaderGetFieldKey(header: Handle, index: u32) -> Key;
    fn AAHeaderGetPayloadSize(header: Handle) -> u64;
    fn AAHeaderGetFieldUInt(header: Handle, index: u32, value: *mut u64) -> i32;
    fn AAHeaderGetFieldString(
        header: Handle,
        index: u32,
        capacity: usize,
        value: *mut libc::c_char,
        length: *mut usize,
    ) -> i32;
    fn AAHeaderGetFieldHash(
        header: Handle,
        index: u32,
        capacity: usize,
        function: *mut u32,
        value: *mut u8,
    ) -> i32;
    fn AAHeaderGetFieldTimespec(header: Handle, index: u32, value: *mut libc::timespec) -> i32;
    fn AAHeaderGetFieldBlob(header: Handle, index: u32, size: *mut u64, offset: *mut u64) -> i32;
    fn AAEntryACLBlobCreateWithEncodedData(data: *const u8, size: usize) -> Handle;
    fn AAEntryACLBlobDestroy(acl: Handle);
    fn AAEntryACLBlobApplyToPath(
        acl: Handle,
        dir: *const libc::c_char,
        path: *const libc::c_char,
        flags: u64,
    ) -> i32;
}
fn ok(status: i32) -> Result<(), String> {
    if status < 0 {
        Err("Ungültiges AppleArchive-Feld".into())
    } else {
        Ok(())
    }
}
struct Header(Handle);
impl Drop for Header {
    fn drop(&mut self) {
        unsafe { AAHeaderDestroy(self.0) };
    }
}
impl Header {
    fn next(reader: &mut impl Read) -> Result<Option<Self>, String> {
        let mut prefix = [0; 6];
        if reader.read(&mut prefix[..1]).map_err(|e| e.to_string())? == 0 {
            return Ok(None);
        }
        reader
            .read_exact(&mut prefix[1..])
            .map_err(|e| e.to_string())?;
        let size = u16::from_le_bytes([prefix[4], prefix[5]]) as usize;
        if &prefix[..4] != b"AA01" || size < 6 {
            return Err("Ungültiger AppleArchive-Header".into());
        }
        let mut bytes = vec![0; size];
        bytes[..6].copy_from_slice(&prefix);
        reader
            .read_exact(&mut bytes[6..])
            .map_err(|e| e.to_string())?;
        let header = unsafe { AAHeaderCreateWithEncodedData(bytes.len(), bytes.as_ptr()) };
        if header.is_null() {
            return Err("AppleArchive-Header nicht lesbar".into());
        }
        let header = Self(header);
        let mut keys = BTreeSet::new();
        for i in 0..unsafe { AAHeaderGetFieldCount(header.0) } {
            if !keys.insert(unsafe { AAHeaderGetFieldKey(header.0, i) }.0) {
                return Err("Doppelte AppleArchive-Felder".into());
            }
        }
        Ok(Some(header))
    }
    fn index(&self, name: &[u8; 3]) -> Result<u32, String> {
        let i = unsafe { AAHeaderGetKeyIndex(self.0, key(name)) };
        if i < 0 {
            Err(format!(
                "AppleArchive-Feld fehlt: {}",
                String::from_utf8_lossy(name)
            ))
        } else {
            Ok(i as u32)
        }
    }
    fn has(&self, name: &[u8; 3]) -> bool {
        self.index(name).is_ok()
    }
    fn uint(&self, name: &[u8; 3]) -> Result<u64, String> {
        let mut v = 0;
        ok(unsafe { AAHeaderGetFieldUInt(self.0, self.index(name)?, &mut v) })?;
        Ok(v)
    }
    fn string(&self, name: &[u8; 3]) -> Result<Vec<u8>, String> {
        let i = self.index(name)?;
        let mut size = 0;
        ok(unsafe { AAHeaderGetFieldString(self.0, i, 0, std::ptr::null_mut(), &mut size) })?;
        if size > 65535 {
            return Err("AppleArchive-Zeichenfolge zu groß".into());
        }
        let mut data = vec![0; size + 1];
        ok(unsafe {
            AAHeaderGetFieldString(self.0, i, data.len(), data.as_mut_ptr().cast(), &mut size)
        })?;
        data.truncate(size);
        Ok(data)
    }
}
fn hash_blob(reader: &mut impl Read, mut remaining: u64) -> Result<String, String> {
    let mut hash = Sha256::new();
    let mut buf = vec![0; 1024 * 1024];
    while remaining > 0 {
        cancelled()?;
        let n = remaining.min(buf.len() as u64) as usize;
        reader
            .read_exact(&mut buf[..n])
            .map_err(|e| e.to_string())?;
        hash.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn xattrs(reader: &mut impl Read, mut remaining: u64) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    while remaining != 0 {
        if remaining < 5 {
            return Err("Abgeschnittene AppleArchive-Attribute".into());
        }
        let mut len = [0; 4];
        reader.read_exact(&mut len).map_err(|e| e.to_string())?;
        let size = u32::from_le_bytes(len) as u64;
        if size < 5 || size > remaining {
            return Err("Ungültige AppleArchive-Attributgröße".into());
        }
        let mut rest = size - 4;
        let mut name = Vec::new();
        loop {
            if rest == 0 || name.len() > 4096 {
                return Err("Ungültiger AppleArchive-Attributname".into());
            }
            let mut b = [0];
            reader.read_exact(&mut b).map_err(|e| e.to_string())?;
            rest -= 1;
            if b[0] == 0 {
                break;
            }
            name.push(b[0]);
        }
        let name = String::from_utf8(name).map_err(|e| e.to_string())?;
        if name.is_empty() || result.insert(name, hash_blob(reader, rest)?).is_some() {
            return Err("Doppeltes oder leeres AppleArchive-Attribut".into());
        }
        remaining -= size;
    }
    Ok(result)
}
fn acl_text(reader: &mut impl Read, size: u64, kind: &str) -> Result<String, String> {
    if size > 1024 * 1024 {
        return Err("AppleArchive-ACL zu groß".into());
    }
    let mut data = vec![0; size as usize];
    reader.read_exact(&mut data).map_err(|e| e.to_string())?;
    let acl = unsafe { AAEntryACLBlobCreateWithEncodedData(data.as_ptr(), data.len()) };
    if acl.is_null() {
        return Err("Ungültige AppleArchive-ACL".into());
    }
    struct Acl(Handle);
    impl Drop for Acl {
        fn drop(&mut self) {
            unsafe { AAEntryACLBlobDestroy(self.0) };
        }
    }
    let acl = Acl(acl);
    let stage = ReadbackDir(PrivateDir::temp()?);
    let path = stage.0 .0.join("metadata");
    match kind {
        "dir" => fs::create_dir(&path),
        "link" => std::os::unix::fs::symlink("missing", &path),
        _ => fs::write(&path, []),
    }
    .map_err(|e| e.to_string())?;
    let dir = CString::new(stage.0 .0.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    ok(unsafe { AAEntryACLBlobApplyToPath(acl.0, dir.as_ptr(), c"metadata".as_ptr(), 0) })?;
    read_acl(&path)
}

pub(super) fn verify(
    reader: &mut impl Read,
    root: &str,
    expected: &[ManifestEntry],
) -> Result<(), String> {
    let _phase =
        crate::work_progress::Phase::enter("Teilarchive: Quelldaten und macOS-Metadaten prüfen");
    let by_path: BTreeMap<String, _> = expected
        .iter()
        .map(|e| {
            let p = if e.p.is_empty() {
                root.to_string()
            } else {
                format!("{root}/{}", e.p)
            };
            (p.nfc().collect(), e)
        })
        .collect();
    if by_path.len() != expected.len() {
        return Err("Kollidierende Quellpfade".into());
    }
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    let mut source_links = BTreeMap::new();
    let mut archive_links = BTreeMap::new();
    while let Some(header) = Header::next(reader)? {
        cancelled()?;
        let path = String::from_utf8(header.string(b"PAT")?).map_err(|e| e.to_string())?;
        let normalized: String = path.nfc().collect();
        let wanted = by_path
            .get(&normalized)
            .ok_or_else(|| format!("Unerwarteter Archiveintrag: {path}"))?;
        if !seen.insert(normalized) {
            return Err(format!("Doppelter Archiveintrag: {path}"));
        }
        let typ = header.uint(b"TYP")?;
        let kind = match typ {
            68 => "dir",
            70 => "file",
            76 => "link",
            _ => return Err(format!("Ungültiger Dateityp: {path}")),
        };
        let mut actual = (*wanted).clone();
        actual.kind = kind.into();
        actual.s = 0;
        actual.hash = if kind == "file" {
            format!("{:x}", Sha256::digest([]))
        } else {
            String::new()
        };
        let mode = header.uint(b"MOD")?;
        if mode > 0o7777 {
            return Err(format!("Ungültiger Zugriffsmodus: {path}"));
        }
        actual.mode = (wanted.mode & !0o7777) | mode as u32;
        actual.flags = u32::try_from(header.uint(b"FLG")?).map_err(|e| e.to_string())?;
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        ok(unsafe { AAHeaderGetFieldTimespec(header.0, header.index(b"MTM")?, &mut time) })?;
        actual.m = time.tv_sec;
        actual.mn = time.tv_nsec;
        actual.link = if header.has(b"LNK") {
            Some(header.string(b"LNK")?)
        } else {
            None
        };
        actual.xattrs.clear();
        actual.acl.clear();
        let cluster = if header.has(b"HLC") {
            Some(header.uint(b"HLC")?)
        } else {
            None
        };
        if kind == "file" {
            // A path-specific identity for entries without HLC prevents silently
            // accepting a missing cluster on one of the expected hard links.
            let id = (
                cluster,
                if cluster.is_none() {
                    path.clone()
                } else {
                    String::new()
                },
            );
            if source_links
                .insert((wanted.dev, wanted.ino), id.clone())
                .is_some_and(|prev| prev != id)
            {
                return Err(format!("Hardlink-Beziehung verloren: {path}"));
            }
            if archive_links
                .insert(id, (wanted.dev, wanted.ino))
                .is_some_and(|prev| prev != (wanted.dev, wanted.ino))
            {
                return Err(format!("Unerwartete Hardlink-Beziehung: {path}"));
            }
        }
        let mut blobs = Vec::new();
        for i in 0..unsafe { AAHeaderGetFieldCount(header.0) } {
            if unsafe { AAHeaderGetFieldType(header.0, i) } == 5 {
                let mut size = 0;
                let mut offset = 0;
                ok(unsafe { AAHeaderGetFieldBlob(header.0, i, &mut size, &mut offset) })?;
                blobs.push((offset, size, unsafe { AAHeaderGetFieldKey(header.0, i) }.0));
            }
        }
        blobs.sort_by_key(|b| b.0);
        let mut position = 0u64;
        for (offset, size, field) in blobs {
            if offset != position {
                return Err("Überlappende oder lückenhafte AppleArchive-Nutzdaten".into());
            }
            position = position
                .checked_add(size)
                .ok_or("AppleArchive-Größenüberlauf")?;
            if field == key(b"DAT").0 {
                if kind != "file" {
                    return Err("Dateidaten auf einem Nicht-Datei-Eintrag".into());
                }
                actual.s = size;
                actual.hash = hash_blob(reader, size)?;
            } else if field == key(b"XAT").0 {
                actual.xattrs = xattrs(reader, size)?;
            } else if field == key(b"ACL").0 {
                actual.acl = acl_text(reader, size, kind)?;
            } else {
                return Err(format!("Unbekannter AppleArchive-Datenblock: {field:x}"));
            }
        }
        if position != unsafe { AAHeaderGetPayloadSize(header.0) } {
            return Err("AppleArchive-Nutzdatengröße stimmt nicht".into());
        }
        if header.has(b"SH2") {
            let mut digest = [0; 32];
            let mut function = 0;
            ok(unsafe {
                AAHeaderGetFieldHash(
                    header.0,
                    header.index(b"SH2")?,
                    32,
                    &mut function,
                    digest.as_mut_ptr(),
                )
            })?;
            let encoded: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            if function != 3 || encoded != actual.hash {
                return Err(format!(
                    "AppleArchive-Inhaltsprüfsumme stimmt nicht: {path}"
                ));
            }
        }
        let mut differences = readback_differences(&actual, wanted);
        // The raw archive must contain the original OS-managed attributes too;
        // extraction-time provenance/quarantine exceptions do not apply here.
        if actual.xattrs != wanted.xattrs {
            differences.push("Erweiterte Attribute im Archiv".into());
        }
        if !differences.is_empty() && problems.len() < 10 {
            problems.push(format!(
                "Archiv-Rückleseprüfung fehlgeschlagen bei {path}: {}",
                differences.join(", ")
            ));
        }
        entries.push(crate::apple_archive::Entry {
            path: PathBuf::from(&path),
            kind: (typ as u8 as char).to_string(),
            link: actual
                .link
                .as_ref()
                .map(|l| String::from_utf8_lossy(l).into_owned()),
            size: actual.s,
            xattr_size: 0,
            acl_size: 0,
            hardlink: cluster,
            hash: Some(actual.hash),
        });
    }
    if seen.len() != expected.len() {
        let missing: Vec<_> = by_path
            .keys()
            .filter(|p| !seen.contains(*p))
            .take(10)
            .collect();
        return Err(format!(
            "Unvollständiges Archiv: {} von {} Einträgen; fehlend: {:?}",
            seen.len(),
            expected.len(),
            missing
        ));
    }
    crate::apple_archive::validate(entries)?;
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    Ok(())
}
