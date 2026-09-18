//! Persist the last successful checksum check, bound to the checked metadata
//! and archive identities. Listing never re-hashes multi-GB archives.
use super::*;
use std::os::unix::fs::MetadataExt;

const RECEIPT: &str = ".verification.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ArchiveIdentity {
    archive: String,
    inode: u64,
    size: u64,
    modified: i64,
    modified_ns: i64,
    changed: i64,
    changed_ns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Evidence {
    metadata_sha256: String,
    archives: Vec<ArchiveIdentity>,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    version: u8,
    verified_at: String,
    evidence: Evidence,
}

fn evidence(root: &Path, metadata: &BackupMetadata) -> Result<Evidence, String> {
    validate_backup_metadata(metadata)?;
    let current = load_backup_metadata(&root.join("metadata.json"))?;
    let serialized = serde_json::to_vec(metadata).map_err(|e| e.to_string())?;
    if serialized != serde_json::to_vec(&current).map_err(|e| e.to_string())? {
        return Err("Backup-Metadaten wurden während der Verifizierung verändert".into());
    }
    let mut archives = Vec::new();
    for item in &metadata.items {
        let stat = fs::symlink_metadata(root.join(&item.archive)).map_err(|e| e.to_string())?;
        if !stat.is_file() {
            return Err(format!("{} ist keine reguläre Archivdatei", item.archive));
        }
        archives.push(ArchiveIdentity {
            archive: item.archive.clone(), inode: stat.ino(), size: stat.len(),
            modified: stat.mtime(), modified_ns: stat.mtime_nsec(),
            changed: stat.ctime(), changed_ns: stat.ctime_nsec(),
        });
    }
    Ok(Evidence { metadata_sha256: format!("{:x}", Sha256::digest(serialized)), archives })
}

/// Invalidate the old result before any new check, including failure/cancel.
pub(super) fn begin(root: &Path, metadata: &BackupMetadata) -> Result<Evidence, String> {
    match fs::remove_file(root.join(RECEIPT)) {
        Ok(()) => crate::throttle::sync_path(root).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(format!("Altes Prüfergebnis konnte nicht zurückgesetzt werden: {e}")),
    }
    evidence(root, metadata)
}

/// Only call after every checksum matched. A storage error is a warning, not
/// a claim that the successfully checked backup contents were corrupt.
pub(super) fn record_success(root: &Path, metadata: &BackupMetadata, before: &Evidence) -> Result<Option<String>, String> {
    if metadata.items.is_empty() { return Ok(None); }
    if evidence(root, metadata)? != *before {
        return Err("Backup-Dateien wurden während der Verifizierung verändert; bitte erneut prüfen".into());
    }
    let receipt = Receipt { version: 1, verified_at: chrono::Utc::now().to_rfc3339(), evidence: before.clone() };
    let bytes = serde_json::to_vec(&receipt).map_err(|e| e.to_string())?;
    Ok(atomic_write(&root.join(RECEIPT), &bytes).err().map(|e|
        format!("Prüfung erfolgreich, aber Prüfergebnis konnte nicht dauerhaft gespeichert werden: {e}")))
}

pub(super) fn is_verified(root: &Path, metadata: &BackupMetadata) -> bool {
    if metadata.items.is_empty() { return false; }
    let Ok(stat) = fs::symlink_metadata(root.join(RECEIPT)) else { return false; };
    if !stat.is_file() { return false; }
    let Ok(bytes) = fs::read(root.join(RECEIPT)) else { return false; };
    let Ok(receipt) = serde_json::from_slice::<Receipt>(&bytes) else { return false; };
    receipt.version == 1 && chrono::DateTime::parse_from_rfc3339(&receipt.verified_at).is_ok()
        && evidence(root, metadata).is_ok_and(|current| current == receipt.evidence)
}
