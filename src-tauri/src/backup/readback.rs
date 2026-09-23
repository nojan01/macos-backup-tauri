//! Verify native archives by fully restoring into an owned temporary directory.
use super::*;

const RESERVE: u64 = 2 * 1024 * 1024 * 1024;

pub(super) fn space_preflight() -> Result<(), String> {
    if crate::ram_parts::ready() {
        Ok(())
    } else {
        require_free_space(&std::env::temp_dir(), crate::segmented::WORK_BYTES)
    }
}

fn full_native_readback(
    archive: &Path,
    root: &str,
    expected: &[ManifestEntry],
) -> Result<Vec<ManifestEntry>, String> {
    let parent = archive
        .parent()
        .ok_or("Archiv ohne übergeordnetes Verzeichnis")?;
    let payload = expected
        .iter()
        .fold(0u64, |total, entry| total.saturating_add(entry.s));
    let required = RESERVE.saturating_add(payload).saturating_add(payload / 10);
    // Full extraction verifies actual file contents and native metadata.
    // Normally the temporary extraction stays on the backup volume. When the
    // user protects the target (throughput limit or gentle sync), prefer the
    // system temp dir if it fits. The target then only sees sequential,
    // limited archive reads instead of writing, statting, hashing and
    // deleting every single file again. Either way the capacity is refused
    // before it is consumed.
    let (stage_parent, label) = match readback_location(parent, required)? {
        ReadbackLocation::Internal(dir) => {
            (dir, "Vollständige Rückleseprüfung über die interne SSD")
        }
        ReadbackLocation::Target => (
            parent.to_path_buf(),
            "Vollständige Rückleseprüfung auf dem Backup-Laufwerk",
        ),
    };
    // Native extraction restores ACLs and immutable flags. Use the cleanup
    // guard that clears those attributes so a failed verification cannot leave
    // a large temporary readback tree behind.
    let stage = ReadbackDir(PrivateDir::new(&stage_parent, ".readback-full")?);
    let _phase = crate::work_progress::Phase::enter(label);
    unpack_private_with_root(archive, &stage.0 .0, Some(std::ffi::OsStr::new(root)))?;
    snapshot_with_phase(&stage.0 .0.join(root), "Vollständige Rückleseprüfung")
}

#[derive(Debug, PartialEq)]
enum ReadbackLocation {
    Internal(PathBuf),
    Target,
}
/// Choose where the full readback tree is extracted, see [`full_native_readback`].
/// Returns an error only when neither location has the required capacity.
fn readback_location(target_parent: &Path, required: u64) -> Result<ReadbackLocation, String> {
    let protected = crate::throttle::limiter_for(target_parent).is_some()
        || crate::throttle::gentle_sync_for(target_parent);
    if protected {
        let internal = std::env::temp_dir();
        let same_volume = std::fs::metadata(&internal)
            .and_then(|i| std::fs::metadata(target_parent).map(|t| i.dev() == t.dev()))
            .unwrap_or(true);
        if !same_volume && require_free_space(&internal, required).is_ok() {
            return Ok(ReadbackLocation::Internal(internal));
        }
    }
    require_free_space(target_parent, required)?;
    Ok(ReadbackLocation::Target)
}

pub(super) fn verify_contents_and_metadata(
    archive: &Path,
    root: &str,
    expected: &[ManifestEntry],
    _stage: &Path,
) -> Result<Vec<ManifestEntry>, String> {
    full_native_readback(archive, root, expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    #[test]
    fn full_readback_stays_on_the_target_unless_it_is_protected_and_internal_space_differs() {
        let owned = ReadbackDir(PrivateDir::temp().unwrap());
        let target = owned.0 .0.join("target");
        fs::create_dir(&target).unwrap();
        assert_eq!(
            readback_location(&target, 1).unwrap(),
            ReadbackLocation::Target
        );
        assert!(
            readback_location(&target, u64::MAX / 4).is_err(),
            "refused before consuming capacity"
        );
        let _throttle = crate::throttle::activate_for_tests(&target, 8).unwrap();
        // The test target shares the system volume with the temp dir, so moving
        // the tree would not relieve the drive and the target is kept.
        assert_eq!(
            readback_location(&target, 1).unwrap(),
            ReadbackLocation::Target
        );
        let same_dev = fs::metadata(std::env::temp_dir()).unwrap().dev()
            == fs::metadata(&target).unwrap().dev();
        assert!(same_dev);
    }
    #[test]
    fn native_readback_checks_contents_metadata_links_and_cleans_stage() {
        let owned = ReadbackDir(PrivateDir::temp().unwrap());
        let source = owned.0 .0.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), b"original data").unwrap();
        xattr::set(
            source.join("file"),
            "com.example.large-metadata",
            b"binary\0metadata\n",
        )
        .unwrap();
        fs::hard_link(source.join("file"), source.join("hardlink")).unwrap();
        let expected = compute_snapshot(&source).unwrap();
        let archive = owned.0 .0.join("archive.aar");
        create_verified_archive_from_snapshot(&source, &archive, &expected).unwrap();
        let stage = owned.0 .0.join("stage");
        fs::create_dir(&stage).unwrap();
        let actual = verify_contents_and_metadata(&archive, "source", &expected, &stage).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(&expected) {
            assert!(readback_differences(a, e).is_empty());
        }
        let file = actual.iter().find(|e| e.p == "file").unwrap();
        let link = actual.iter().find(|e| e.p == "hardlink").unwrap();
        assert_eq!(file.ino, link.ino);
        assert_eq!(fs::read_dir(&stage).unwrap().count(), 0);
        assert!(!fs::read_dir(&owned.0 .0).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".readback-full")));
        let mut wrong = expected.clone();
        wrong.iter_mut().find(|e| e.p == "file").unwrap().hash = "0".repeat(64);
        let actual = verify_contents_and_metadata(&archive, "source", &wrong, &stage).unwrap();
        assert!(actual
            .iter()
            .zip(&wrong)
            .any(|(a, e)| readback_differences(a, e)
                .iter()
                .any(|d| d.contains("SHA-256"))));
        fs::write(&archive, b"corrupt archive").unwrap();
        assert!(verify_contents_and_metadata(&archive, "source", &expected, &stage).is_err());
        assert_eq!(fs::read_dir(&stage).unwrap().count(), 0);
    }
    #[test]
    fn binary_multiline_native_metadata_roundtrips_backup_and_restore() {
        let owned = ReadbackDir(PrivateDir::temp().unwrap());
        let source = owned.0 .0.join("pax-source");
        fs::create_dir(&source).unwrap();
        let name = format!("{}\nsecond-line", "long-name-".repeat(16));
        let path = source.join(&name);
        fs::write(&path, b"original payload").unwrap();
        let value = b"binary\0value\nsecond line\xff=end";
        xattr::set(&path, "com.example.backup-pax", value).unwrap();
        let expected = compute_snapshot(&source).unwrap();
        let archive = owned.0 .0.join("pax.aar");
        create_verified_archive_from_snapshot(&source, &archive, &expected).unwrap();
        let destination = owned.0 .0.join("restored");
        fs::create_dir(&destination).unwrap();
        unpack_private(&archive, &destination).unwrap();
        let restored = destination.join("pax-source").join(&name);
        assert_eq!(fs::read(&restored).unwrap(), b"original payload");
        assert_eq!(
            xattr::get(&restored, "com.example.backup-pax")
                .unwrap()
                .unwrap(),
            value
        );
        let actual = compute_snapshot(&destination.join("pax-source")).unwrap();
        for (a, e) in actual.iter().zip(&expected) {
            assert!(readback_differences(a, e).is_empty());
        }
        assert_eq!(actual.len(), expected.len());
    }
    #[test]
    fn native_readback_keeps_exact_nested_entry_set() {
        let owned = ReadbackDir(PrivateDir::temp().unwrap());
        let source = owned.0 .0.join("Documents");
        fs::create_dir_all(source.join("GitHub/project/target/debug/build/empty-output")).unwrap();
        fs::create_dir_all(source.join("GitHub/other-project/src-tauri")).unwrap();
        fs::write(source.join("GitHub/project/README.md"), b"read me").unwrap();
        fs::write(
            source.join("GitHub/other-project/src-tauri/Info.plist"),
            b"plist",
        )
        .unwrap();
        let long = format!("GitHub/{}", "pax-directory-".repeat(12));
        fs::create_dir_all(source.join(&long)).unwrap();
        fs::write(source.join(&long).join("long-file.txt"), b"long path").unwrap();
        xattr::set(
            source.join("GitHub/project/README.md"),
            "com.example.backup-pax",
            b"binary\0value\nsecond line",
        )
        .unwrap();
        xattr::set(
            source.join(&long).join("long-file.txt"),
            "com.example.backup-pax",
            b"long PAX path metadata",
        )
        .unwrap();
        let changed = source.join("GitHub/project");
        let name = CString::new(changed.as_os_str().as_bytes()).unwrap();
        let times = [libc::timespec {
            tv_sec: 1_700_000_000,
            tv_nsec: 123_456_789,
        }; 2];
        assert_eq!(
            unsafe { libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), 0) },
            0
        );
        let expected = compute_snapshot(&source).unwrap();
        let archive = owned.0 .0.join("documents.aar");
        create_verified_archive_from_snapshot(&source, &archive, &expected).unwrap();
        let stage = owned.0 .0.join("readback");
        fs::create_dir(&stage).unwrap();
        let actual =
            verify_contents_and_metadata(&archive, "Documents", &expected, &stage).unwrap();
        assert_eq!(
            actual
                .iter()
                .map(|entry| entry.p.as_str())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|entry| entry.p.as_str())
                .collect::<Vec<_>>(),
        );
        for (actual, expected) in actual.iter().zip(&expected) {
            assert!(
                readback_differences(actual, expected).is_empty(),
                "{:?}",
                readback_differences(actual, expected)
            );
        }
    }
    #[test]
    fn sparse_payload_is_fully_verified_and_temporary_copy_removed() {
        let owned = ReadbackDir(PrivateDir::temp().unwrap());
        let source = owned.0 .0.join("virtual-disk.hds");
        let mut file = fs::File::create(&source).unwrap();
        file.set_len(64 * 1024 * 1024).unwrap();
        file.write_all(b"nonzero beginning of sparse virtual disk")
            .unwrap();
        drop(file);
        xattr::set(&source, "com.example.backup", b"must survive").unwrap();
        let expected = compute_snapshot(&source).unwrap();
        let archive = owned.0 .0.join("disk.aar");
        create_verified_archive_from_snapshot(&source, &archive, &expected).unwrap();
        let stage = owned.0 .0.join("readback");
        fs::create_dir(&stage).unwrap();
        let actual =
            verify_contents_and_metadata(&archive, "virtual-disk.hds", &expected, &stage).unwrap();
        assert!(readback_differences(&actual[0], &expected[0]).is_empty());
        assert_eq!(fs::read_dir(&stage).unwrap().count(), 0);
        let allocated: u64 = WalkDir::new(&stage)
            .into_iter()
            .map(|e| fs::symlink_metadata(e.unwrap().path()).unwrap().blocks() * 512)
            .sum();
        assert!(
            allocated < 2 * 1024 * 1024,
            "temporary allocation: {allocated}"
        );
        assert_eq!(fs::metadata(&source).unwrap().len(), 64 * 1024 * 1024);
    }
    #[test]
    #[ignore = "manual 64-bit size and sparse restore integration; reads more than 4 GiB several times"]
    fn native_archive_restores_sparse_file_larger_than_four_gib() {
        use std::io::{Seek, SeekFrom};
        let owned = ReadbackDir(PrivateDir::temp().unwrap());
        let source = owned.0 .0.join("large.hds");
        let size = (1u64 << 32) + 8192;
        let mut file = fs::File::create(&source).unwrap();
        file.set_len(size).unwrap();
        file.write_all(b"start of virtual disk").unwrap();
        file.seek(SeekFrom::End(-16)).unwrap();
        file.write_all(b"end of disk data").unwrap();
        drop(file);
        let expected = compute_snapshot(&source).unwrap();
        let archive = owned.0 .0.join("large.aar");
        create_verified_archive_from_snapshot(&source, &archive, &expected).unwrap();
        let output = owned.0 .0.join("restore");
        fs::create_dir(&output).unwrap();
        unpack_private(&archive, &output).unwrap();
        let restored = output.join("large.hds");
        let actual = compute_snapshot(&restored).unwrap();
        assert_eq!(actual[0].s, size);
        assert!(readback_differences(&actual[0], &expected[0]).is_empty());
        assert!(fs::metadata(&restored).unwrap().blocks() * 512 < size / 10);
        println!("LZFSE_LARGE_SPARSE_ROUNDTRIP_PASSED bytes={size}");
    }
    #[test]
    fn same_size_wrong_payload_is_rejected() {
        let owned = ReadbackDir(PrivateDir::temp().unwrap());
        let source = owned.0 .0.join("file");
        fs::write(&source, b"original").unwrap();
        let expected = compute_snapshot(&source).unwrap();
        fs::write(&source, b"tampered").unwrap();
        let archive = owned.0 .0.join("changed.aar");
        create_verified_archive(&source, &archive).unwrap();
        assert!(verify_archive_source(&archive, "file", &expected)
            .unwrap_err()
            .contains("SHA-256"));
    }
}
