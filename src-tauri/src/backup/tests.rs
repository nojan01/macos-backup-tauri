use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};
fn fixture() -> PrivateDir {
    BACKUP_CANCELLED.store(false, Ordering::SeqCst);
    VERIFY_CANCELLED.store(false, Ordering::SeqCst);
    PrivateDir::temp().unwrap()
}
fn set_times(path: &Path, sec: i64, nano: i64) {
    let p = CString::new(path.as_os_str().as_bytes()).unwrap();
    let times = [libc::timespec {
        tv_sec: sec,
        tv_nsec: nano,
    }; 2];
    assert_eq!(
        unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                p.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        },
        0
    );
}
#[test]
fn content_change_with_same_length_and_exact_mtime_is_detected() {
    let d = fixture();
    let p = d.0.join("file");
    fs::write(&p, b"aaaa").unwrap();
    set_times(&p, 1700000000, 123);
    let before = compute_snapshot(&p).unwrap();
    fs::write(&p, b"bbbb").unwrap();
    set_times(&p, 1700000000, 123);
    assert_ne!(before[0].hash, compute_snapshot(&p).unwrap()[0].hash);
    assert!(ensure_unchanged(&p, &before).is_err());
}
#[test]
fn permissions_and_empty_directories_are_detected() {
    let d = fixture();
    let p = d.0.join("tree");
    fs::create_dir(&p).unwrap();
    fs::write(p.join("file"), b"x").unwrap();
    let a = compute_snapshot(&p).unwrap();
    fs::set_permissions(p.join("file"), fs::Permissions::from_mode(0o600)).unwrap();
    assert_ne!(a, compute_snapshot(&p).unwrap());
    let b = compute_snapshot(&p).unwrap();
    fs::create_dir(p.join("empty")).unwrap();
    assert_ne!(b, compute_snapshot(&p).unwrap());
    fs::remove_dir(p.join("empty")).unwrap();
    assert_eq!(compute_snapshot(&p).unwrap().len(), 2);
}
#[test]
fn symlink_targets_are_scanned_without_following_even_at_root() {
    let d = fixture();
    let p = d.0.join("link");
    symlink("missing-a", &p).unwrap();
    let a = compute_snapshot(&p).unwrap();
    fs::remove_file(&p).unwrap();
    symlink("missing-b", &p).unwrap();
    let b = compute_snapshot(&p).unwrap();
    assert_ne!(a[0].link, b[0].link);
    assert_eq!(b.len(), 1);
}
#[test]
fn missing_unreadable_and_special_sources_fail() {
    let d = fixture();
    assert!(compute_snapshot(&d.0.join("missing")).is_err());
    let p = d.0.join("locked");
    fs::create_dir(&p).unwrap();
    fs::write(p.join("secret"), b"x").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0)).unwrap();
    let result = compute_snapshot(&p);
    fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    let fifo = d.0.join("fifo");
    let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(compute_snapshot(&fifo).is_err());
}
#[test]
fn archived_tree_includes_previously_excluded_names_and_roundtrips_metadata() {
    let d = fixture();
    let p = d.0.join("Logs");
    fs::create_dir(&p).unwrap();
    for name in ["node_modules", "Caches", "empty"] {
        fs::create_dir(p.join(name)).unwrap();
    }
    fs::write(p.join("node_modules/data"), b"essential").unwrap();
    fs::write(p.join("Caches/data"), b"cached").unwrap();
    fs::write(p.join(".DS_Store"), b"finder").unwrap();
    fs::write(p.join("-weird\n[1]*"), b"bytes\0\xff").unwrap();
    fs::hard_link(p.join("Caches/data"), p.join("hardlink")).unwrap();
    symlink("Caches/data", p.join("link")).unwrap();
    symlink("absent", p.join("dangling")).unwrap();
    xattr::set(
        p.join("Caches/data"),
        "com.example.backup-audit",
        b"attribute",
    )
    .unwrap();
    fs::set_permissions(p.join("Caches/data"), fs::Permissions::from_mode(0o600)).unwrap();
    let archive = d.0.join("archive.tar.gz");
    create_verified_archive(&p, &archive, true).unwrap();
    verify_archive_source(&archive, "Logs", &compute_snapshot(&p).unwrap()).unwrap();
}
#[test]
fn single_file_preserves_xattrs_and_nanoseconds() {
    let d = fixture();
    let p = d.0.join("data");
    fs::write(&p, b"payload").unwrap();
    xattr::set(&p, "com.example.backup-audit", b"extended value").unwrap();
    set_times(&p, 1700000000, 123456789);
    let a = d.0.join("file.tar.gz");
    create_file_archive(&p, "data", &a).unwrap();
    verify_archive_source(&a, "data", &compute_snapshot(&p).unwrap()).unwrap();
}
#[test]
fn acl_changes_are_detected_and_preserved() {
    let d = fixture();
    let p = d.0.join("data");
    fs::write(&p, b"payload").unwrap();
    let before = compute_snapshot(&p).unwrap();
    let out = Command::new("/bin/chmod")
        .args(["+a", "everyone deny delete"])
        .arg(&p)
        .output()
        .unwrap();
    assert!(out.status.success());
    let after = compute_snapshot(&p).unwrap();
    assert_ne!(before[0].acl, after[0].acl);
    create_file_archive(&p, "data", &d.0.join("file.tar.gz")).unwrap();
}
#[test]
fn target_inside_source_or_backup_as_source_is_rejected() {
    let d = fixture();
    let source = d.0.join("source");
    fs::create_dir(&source).unwrap();
    assert!(validate_source_target(&source, &source.join("nested/target")).is_err());
    symlink(&source, d.0.join("alias")).unwrap();
    assert!(validate_source_target(&source, &d.0.join("alias/new")).is_err());
    assert!(validate_source_target(&source, &d.0.join("elsewhere")).is_ok());
}
#[test]
fn legacy_weak_manifest_cannot_be_reused() {
    let d = fixture();
    fs::create_dir(d.0.join("manifests")).unwrap();
    fs::write(
        manifest_path_for(&d.0, "data.tar.gz"),
        br#"[{"p":"file","s":4,"m":1700000000}]"#,
    )
    .unwrap();
    assert!(load_manifest(&d.0, "data.tar.gz").is_none());
}
#[test]
fn manifests_and_checkpoints_are_durable_and_resume_replaces_old_versions() {
    let d = fixture();
    let p = d.0.join("file");
    fs::write(&p, b"one").unwrap();
    let snapshot = compute_snapshot(&p).unwrap();
    save_manifest(&d.0, "data.tar.gz", &snapshot).unwrap();
    assert_eq!(load_manifest(&d.0, "data.tar.gz").unwrap(), snapshot);
    let mut item = BackupItem {
        path: "~/file".into(),
        archive: "data.tar.gz".into(),
        hash: "a".repeat(64),
        archive_size_bytes: 1,
        source_size_bytes: 3,
    };
    append_resume_entry(&d.0, &item).unwrap();
    item.hash = "b".repeat(64);
    append_resume_entry(&d.0, &item).unwrap();
    let entries = load_resume_entries(&d.0);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hash, item.hash);
}
#[test]
fn failed_publication_keeps_old_archive_and_hardlink_reuse_does_not_truncate_it() {
    let d = fixture();
    let old = d.0.join("old");
    let target = d.0.join("target");
    fs::write(&old, b"previous").unwrap();
    fs::hard_link(&old, &target).unwrap();
    assert!(create_verified_archive(&d.0.join("missing"), &target, true).is_err());
    assert_eq!(fs::read(&old).unwrap(), b"previous");
    let newer = d.0.join("newer");
    fs::write(&newer, b"new").unwrap();
    reuse_archive(&newer, &target).unwrap();
    assert_eq!(fs::read(&old).unwrap(), b"previous");
    assert_eq!(fs::read(&target).unwrap(), b"new");
}
#[test]
fn cancellation_never_publishes_successful_archive() {
    let d = fixture();
    let p = d.0.join("file");
    fs::write(&p, b"data").unwrap();
    let a = d.0.join("archive");
    BACKUP_CANCELLED.store(true, Ordering::SeqCst);
    let r = create_verified_archive(&p, &a, true);
    BACKUP_CANCELLED.store(false, Ordering::SeqCst);
    assert!(r.is_err());
    assert!(!a.exists());
}
#[test]
fn corrupt_archive_fails_readback() {
    let d = fixture();
    let p = d.0.join("file");
    fs::write(&p, b"data").unwrap();
    let a = d.0.join("archive");
    fs::write(&a, b"not an archive").unwrap();
    assert!(verify_archive_source(&a, "file", &compute_snapshot(&p).unwrap()).is_err());
}
fn completed_fixture(d: &PrivateDir) -> (PathBuf, PathBuf, BackupMetadata, Vec<ManifestEntry>) {
    let source = d.0.join("Documents");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("data"), b"backup-data").unwrap();
    let backup = d.0.join("backup");
    fs::create_dir(&backup).unwrap();
    let a = backup.join("docs.tar.gz");
    create_verified_archive(&source, &a, true).unwrap();
    let metadata = BackupMetadata {
        timestamp: "20260907-120000".into(),
        items: vec![BackupItem {
            path: "~/Documents".into(),
            archive: "docs.tar.gz".into(),
            hash: hash_file(&a).unwrap(),
            archive_size_bytes: fs::metadata(&a).unwrap().len(),
            source_size_bytes: 11,
        }],
        hash_algorithm: "sha256".into(),
        total_source_size_bytes: 11,
        start_time: "".into(),
        end_time: "".into(),
        duration_seconds: 0,
    };
    let expected = compute_snapshot(&source).unwrap();
    (source, backup, metadata, expected)
}
#[test]
fn actual_backup_finalization_and_restore_roundtrip() {
    let d = fixture();
    let (source, backup, meta, expected) = completed_fixture(&d);
    finish_backup(&backup, &meta, &[(source, expected)]).unwrap();
    let home = d.0.join("restored");
    fs::create_dir(&home).unwrap();
    let restored = restore_selected(&backup, &["~/Documents".into()], true, &home, None).unwrap();
    assert_eq!(restored.restored_count, 1);
    assert_eq!(
        fs::read(home.join("Documents/data")).unwrap(),
        b"backup-data"
    );
}
#[test]
fn changed_or_deleted_sources_block_completion_even_after_archive_was_written() {
    let d = fixture();
    let (source, backup, meta, expected) = completed_fixture(&d);
    append_resume_entry(&backup, &meta.items[0]).unwrap();
    fs::write(source.join("data"), b"changed-now").unwrap();
    assert!(finish_backup(&backup, &meta, &[(source.clone(), expected.clone())]).is_err());
    assert!(!backup.join("metadata.json").exists());
    assert!(resume_state_path(&backup).exists());
    fs::remove_dir_all(&source).unwrap();
    assert!(finish_backup(&backup, &meta, &[(source, expected)]).is_err());
    assert!(!backup.join("metadata.json").exists());
}
#[test]
fn damaged_archive_or_metadata_write_failure_cannot_complete_backup() {
    let d = fixture();
    let (source, backup, meta, expected) = completed_fixture(&d);
    fs::create_dir(backup.join("metadata.json")).unwrap();
    assert!(finish_backup(&backup, &meta, &[(source.clone(), expected.clone())]).is_err());
    assert!(load_backup_metadata(&backup.join("metadata.json")).is_err());
    fs::remove_dir(backup.join("metadata.json")).unwrap();
    fs::write(backup.join("docs.tar.gz"), b"corrupt").unwrap();
    assert!(finish_backup(&backup, &meta, &[(source, expected)]).is_err());
    assert!(!backup.join("metadata.json").exists());
}
#[test]
fn resumed_archive_is_replaced_with_current_source_and_old_backup_survives() {
    let d = fixture();
    let (source, backup, mut meta, _) = completed_fixture(&d);
    let archive = backup.join("docs.tar.gz");
    let previous = d.0.join("previous.tar.gz");
    fs::hard_link(&archive, &previous).unwrap();
    let old_hash = hash_file(&previous).unwrap();
    append_resume_entry(&backup, &meta.items[0]).unwrap();
    fs::write(source.join("data"), b"new-version").unwrap();
    create_verified_archive(&source, &archive, true).unwrap();
    meta.items[0].hash = hash_file(&archive).unwrap();
    meta.items[0].archive_size_bytes = fs::metadata(&archive).unwrap().len();
    append_resume_entry(&backup, &meta.items[0]).unwrap();
    finish_backup(
        &backup,
        &meta,
        &[(source.clone(), compute_snapshot(&source).unwrap())],
    )
    .unwrap();
    assert_eq!(hash_file(&previous).unwrap(), old_hash);
    let home = d.0.join("restored");
    fs::create_dir(&home).unwrap();
    restore_selected(&backup, &["~/Documents".into()], true, &home, None).unwrap();
    assert_eq!(
        fs::read(home.join("Documents/data")).unwrap(),
        b"new-version"
    );
}
#[test]
fn empty_and_corrupt_metadata_checkpoints_remain_resumable() {
    let d = fixture();
    let root = d.0.join("macos-backup-suite/data/20260907-120000");
    fs::create_dir_all(&root).unwrap();
    atomic_write(&resume_state_path(&root), b"").unwrap();
    assert_eq!(
        list_resumable_backups(d.0.to_str().unwrap().into())
            .unwrap()
            .len(),
        1
    );
    fs::write(root.join("metadata.json"), b"{truncated").unwrap();
    assert_eq!(
        list_resumable_backups(d.0.to_str().unwrap().into())
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn whole_home_backup_restores_and_test_restores_for_a_different_user_name() {
    let d = fixture();
    let (source, backup, mut meta, expected) = completed_fixture(&d);
    meta.items[0].path = "~".into();
    finish_backup(&backup, &meta, &[(source, expected)]).unwrap();
    let home = d.0.join("another-user");
    fs::create_dir(&home).unwrap();
    let r = restore_selected(&backup, &["~".into()], true, &home, None).unwrap();
    assert_eq!(r.restored_count, 1);
    assert_eq!(fs::read(home.join("data")).unwrap(), b"backup-data");
    let test_dest = d.0.join("test-output");
    fs::create_dir(&test_dest).unwrap();
    let r = test_restore_to(&backup, "~", &test_dest).unwrap();
    assert_eq!(r.file_count, 1);
}
#[test]
fn insufficient_space_is_an_explicit_error() {
    let d = fixture();
    assert!(require_free_space(&d.0, u64::MAX - 16 * 1024 * 1024).is_err());
}
#[test]
fn overwrite_restores_metadata_of_existing_directories() {
    let d = fixture();
    let source = d.0.join("Documents");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), b"data").unwrap();
    xattr::set(&source, "com.example.directory", b"attribute").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
    set_times(&source, 1700000000, 123456789);
    let a = d.0.join("archive");
    create_verified_archive(&source, &a, true).unwrap();
    let target = d.0.join("home/Documents");
    fs::create_dir_all(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    staged_restore(&a, &target, false).unwrap();
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o755);
    staged_restore(&a, &target, true).unwrap();
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        xattr::get(&target, "com.example.directory")
            .unwrap()
            .unwrap(),
        b"attribute"
    );
    assert_eq!(fs::metadata(&target).unwrap().mtime(), 1700000000);
    assert_eq!(fs::metadata(&target).unwrap().mtime_nsec(), 123456789);
}
#[test]
fn readback_cleans_immutable_files_without_changing_source() {
    let d = fixture();
    let p = d.0.join("file");
    fs::write(&p, b"data").unwrap();
    let path = CString::new(p.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::chflags(path.as_ptr(), 2) }, 0);
    let result = create_file_archive(&p, "file", &d.0.join("archive"));
    assert_eq!(unsafe { libc::chflags(path.as_ptr(), 0) }, 0);
    result.unwrap();
    let private = PrivateDir::temp().unwrap();
    let cleanup_path = private.0.clone();
    let locked = cleanup_path.join("locked");
    fs::write(&locked, b"test").unwrap();
    let name = CString::new(locked.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::chflags(name.as_ptr(), 2) }, 0);
    drop(ReadbackDir(private));
    assert!(!cleanup_path.exists());
}

#[test]
fn large_file_scan_reports_progress_inside_file_and_matches_manifest() {
    let d=fixture();let path=d.0.join("large.bin");let payload=vec![0x5a;5*1024*1024+17];fs::write(&path,&payload).unwrap();
    let mut events=Vec::new();let snapshot=scan_with_activity(&path,&mut |a|events.push(a.clone())).unwrap();
    assert!(events.iter().any(|a| a.bytes>0 && a.bytes<(payload.len() as u64)));
    assert_eq!(events.last().unwrap().bytes,payload.len() as u64);
    assert!(events.windows(2).all(|p|p[0].bytes<=p[1].bytes));
    assert_eq!(snapshot,compute_snapshot(&path).unwrap());
}
#[test]
fn progress_does_not_prevent_cancelling_inside_large_file() {
    let d=fixture();let path=d.0.join("large.bin");fs::write(&path,vec![0x5a;3*1024*1024]).unwrap();
    let result=scan_with_activity(&path,&mut |a| {if a.bytes>0 {BACKUP_CANCELLED.store(true,Ordering::SeqCst);}});
    BACKUP_CANCELLED.store(false,Ordering::SeqCst);assert!(result.unwrap_err().contains("abgebrochen"));
}
#[test]
fn accelerated_sha256_matches_known_digest() {
    assert_eq!(format!("{:x}",Sha256::digest(b"abc")),"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}


#[test]
fn git_runtime_socket_does_not_block_archiving_or_restore() {
    let _guard=OperationGuard::acquire().unwrap();
    let d=PrivateDir::new(Path::new("/tmp"),"socket-test").unwrap();
    let source=d.0.join("project");fs::create_dir_all(source.join(".git")).unwrap();
    fs::write(source.join(".git/HEAD"),b"ref: refs/heads/main\n").unwrap();
    fs::write(source.join("ordinary.sock"),b"important file content").unwrap();
    let socket=source.join(".git/fsmonitor--daemon.ipc");
    let _listener=std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let mut skipped=std::collections::BTreeSet::new();
    let snapshot=scan_with_activity(&source,&mut |a| {if a.skipped_socket {skipped.insert(a.current_file.clone());}}).unwrap();
    assert_eq!(skipped,std::collections::BTreeSet::from([socket.clone()]));
    assert!(!snapshot.iter().any(|e|e.p.ends_with("fsmonitor--daemon.ipc")));
    assert!(snapshot.iter().any(|e|e.p=="ordinary.sock"));
    let archive=d.0.join("project.tar.gz");create_verified_archive(&source,&archive,true).unwrap();
    let target=d.0.join("restored/project");fs::create_dir(target.parent().unwrap()).unwrap();staged_restore(&archive,&target,true).unwrap();
    assert_eq!(fs::read(target.join(".git/HEAD")).unwrap(),b"ref: refs/heads/main\n");
    assert_eq!(fs::read(target.join("ordinary.sock")).unwrap(),b"important file content");
    assert!(!target.join(".git/fsmonitor--daemon.ipc").exists());
    let audit:serde_json::Value=serde_json::from_slice(&socket_report_json(&skipped).unwrap()).unwrap();
    assert_eq!(audit["skipped_unix_sockets"][0],socket.to_str().unwrap());
}
#[test]
fn a_regular_file_named_like_git_socket_is_still_backed_up() {
    let d=fixture();let root=d.0.join("project");fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/fsmonitor--daemon.ipc"),b"regular data").unwrap();
    let snapshot=compute_snapshot(&root).unwrap();assert!(snapshot.iter().any(|e|e.p==".git/fsmonitor--daemon.ipc"&&e.kind=="file"));
    create_verified_archive(&root,&d.0.join("archive"),true).unwrap();
}
#[test]
fn explicit_socket_source_and_nested_fifo_remain_errors() {
    let _guard=OperationGuard::acquire().unwrap();let d=PrivateDir::new(Path::new("/tmp"),"socket-test").unwrap();
    let socket=d.0.join("endpoint");let _listener=std::os::unix::net::UnixListener::bind(&socket).unwrap();
    assert!(compute_snapshot(&socket).unwrap_err().contains("Laufzeit-Socket"));
    let root=d.0.join("data");fs::create_dir(&root).unwrap();let fifo=root.join("pipe");let name=CString::new(fifo.as_os_str().as_bytes()).unwrap();assert_eq!(unsafe{libc::mkfifo(name.as_ptr(),0o600)},0);
    assert!(compute_snapshot(&root).unwrap_err().contains("FIFO"));
}

#[test]
fn archive_validation_and_restore_merge_honor_cancellation() {
    let d=fixture();let source=d.0.join("source");fs::write(&source,b"data").unwrap();let archive=d.0.join("archive");create_verified_archive(&source,&archive,true).unwrap();
    cancel_operation().unwrap();
    assert!(archive_index(&archive).unwrap_err().contains("abgebrochen"));
    let target=d.0.join("target");assert!(merge_tree(&source,&target,true).unwrap_err().contains("abgebrochen"));assert!(!target.exists());assert!(source.exists());
    BACKUP_CANCELLED.store(false,Ordering::SeqCst);VERIFY_CANCELLED.store(false,Ordering::SeqCst);
}

#[test]
fn preflight_reports_all_missing_roots_before_deep_scan() {
    let d=fixture();let existing=d.0.join("Documents");fs::create_dir(&existing).unwrap();
    // A deep scan would reject this FIFO first; shallow preflight must instead
    // inspect the remaining roots and report both stale selections together.
    let fifo=existing.join("pipe");let name=CString::new(fifo.as_os_str().as_bytes()).unwrap();assert_eq!(unsafe{libc::mkfifo(name.as_ptr(),0o600)},0);
    let selected=vec![existing.to_str().unwrap().into(),"~/Library/Application Support/Code/User".into(),"~/another-missing".into()];
    let error=validate_selected_sources(&selected,&d.0.join("destination"),&d.0).unwrap_err();
    assert!(error.contains("Code/User"));assert!(error.contains("another-missing"));assert!(!error.contains("FIFO"));assert!(error.contains("noch keine Dateiinhalte"));
    assert!(!d.0.join("destination").exists());
}
#[test]
fn preflight_accepts_existing_roots_and_dangling_links() {
    let d=fixture();let dir=d.0.join("Documents");fs::create_dir(&dir).unwrap();let file=d.0.join("config");fs::write(&file,b"data").unwrap();let link=d.0.join("link");symlink("absent-target",&link).unwrap();
    let selected=vec!["~/Documents".into(),"~/config".into(),"~/link".into()];
    validate_selected_sources(&selected,&d.0.join("destination"),&d.0).unwrap();
    assert!(compute_snapshot(&link).is_ok());
}
#[test]
fn preflight_rejects_unreadable_roots_overlap_and_duplicates() {
    let d=fixture();let dir=d.0.join("Documents");fs::create_dir(&dir).unwrap();
    let selected=vec!["~/Documents".into()];assert!(validate_selected_sources(&selected,&dir.join("backup"),&d.0).unwrap_err().contains("ineinander"));
    let duplicate=vec!["~/Documents".into(),dir.to_str().unwrap().into()];assert!(validate_selected_sources(&duplicate,&d.0.join("dest"),&d.0).unwrap_err().contains("mehrfach"));
    fs::set_permissions(&dir,fs::Permissions::from_mode(0)).unwrap();let result=validate_selected_sources(&selected,&d.0.join("dest"),&d.0);fs::set_permissions(&dir,fs::Permissions::from_mode(0o700)).unwrap();assert!(result.unwrap_err().contains("nicht lesbar"));
}
