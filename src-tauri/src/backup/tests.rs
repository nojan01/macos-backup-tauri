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

#[test]
#[ignore = "manual representative backup benchmark"]
fn representative_backup_roundtrip() {
    let d=fixture();let source=d.0.join("Documents");fs::create_dir(&source).unwrap();
    let data=vec![37u8;1024*1024];
    for i in 0..1500 {
        let sub=source.join(format!("package-{i}"));fs::create_dir(&sub).unwrap();
        fs::write(sub.join("settings.json"),b"{\"test\":true}").unwrap();
        xattr::set(&sub,"com.example.backup-benchmark",b"attribute").unwrap();
    }
    for i in 0..64 {fs::write(source.join(format!("data-{i}")),&data).unwrap();}
    let start=std::time::Instant::now();
    let initial=compute_snapshot(&source).unwrap();
    let archive=d.0.join("documents.tar.gz");
    create_verified_archive_from_snapshot(&source,&archive,true,&initial).unwrap();
    let _hash=hash_file(&archive).unwrap();
    ensure_unchanged(&source,&initial).unwrap();
    println!("REPRESENTATIVE_BACKUP {:.3}s {} entries",start.elapsed().as_secs_f64(),initial.len());
}

#[test]
fn cached_source_baseline_rejects_changed_added_and_removed_files() {
    for change in ["changed", "added", "removed"] {
        let d=fixture();let source=d.0.join("source");fs::create_dir(&source).unwrap();let file=source.join("file");fs::write(&file,b"original").unwrap();
        let baseline=compute_snapshot(&source).unwrap();
        match change {
            "changed" => fs::write(&file,b"modified").unwrap(),
            "added" => fs::write(source.join("new"),b"new data").unwrap(),
            _ => fs::remove_file(&file).unwrap(),
        }
        let target=d.0.join("archive.tar.gz");
        assert!(create_verified_archive_from_snapshot(&source,&target,true,&baseline).is_err(),"{change} source accepted");
        assert!(!target.exists(),"{change} source published");
    }
}
#[test]
fn single_validation_unpack_still_rejects_wrong_root_before_writing() {
    let d=fixture();let source=d.0.join("actual");fs::write(&source,b"payload").unwrap();let archive=d.0.join("archive.tar.gz");create_verified_archive(&source,&archive,true).unwrap();
    let stage=PrivateDir::temp().unwrap();
    assert!(unpack_private_with_root(&archive,&stage.0,Some(std::ffi::OsStr::new("wrong"))).is_err());
    assert!(fs::read_dir(&stage.0).unwrap().next().is_none());
}

#[test]
fn regenerated_provenance_is_allowed_only_in_readback_not_source_guards() {
    let d=fixture();let source=d.0.join("Documents");fs::create_dir(&source).unwrap();fs::write(source.join("data"),b"must survive").unwrap();
    xattr::set(&source,"com.example.required",b"keep this metadata").unwrap();
    let archive=d.0.join("archive.tar.gz");create_verified_archive(&source,&archive,true).unwrap();
    let mut expected=compute_snapshot(&source).unwrap();
    for entry in &mut expected {entry.xattrs.insert("com.apple.provenance".into(),"different source application".into());}
    verify_archive_source(&archive,"Documents",&expected).unwrap();
    assert!(ensure_unchanged(&source,&expected).is_err());
    expected[0].xattrs.insert("com.example.required".into(),"missing or corrupt metadata".into());
    let error=verify_archive_source(&archive,"Documents",&expected).unwrap_err();
    assert!(error.contains("bei Documents:"));assert!(error.contains("com.example.required"));
}
#[test]
fn provenance_exception_never_hides_content_acl_resource_forks_or_other_xattrs() {
    let d=fixture();let file=d.0.join("file");fs::write(&file,b"contents").unwrap();
    let original=compute_snapshot(&file).unwrap().remove(0);
    for field in ["hash","acl","com.apple.ResourceFork","com.apple.FinderInfo","com.apple.macl","com.apple.quarantine"] {
        let mut changed=original.clone();
        changed.xattrs.insert("com.apple.provenance".into(),"regenerated".into());
        match field {"hash"=>changed.hash="bad".into(),"acl"=>changed.acl="different".into(),_=>{changed.xattrs.insert(field.into(),"bad".into());}}
        assert!(!readback_differences(&changed,&original).is_empty(),"{field} mismatch hidden");
    }
}

#[test]
#[ignore = "manual read-only metadata probe; requires BACKUP_PROBE_ROOT"]
fn actual_directory_root_metadata_roundtrip() {
    let root=PathBuf::from(std::env::var("BACKUP_PROBE_ROOT").expect("Explicit root required"));
    let md=fs::symlink_metadata(&root).unwrap();assert!(md.is_dir());
    let mut attrs=BTreeMap::new();
    for key in xattr::list(&root).unwrap() {attrs.insert(key.to_str().unwrap().to_string(),format!("{:x}",Sha256::digest(xattr::get(&root,&key).unwrap().unwrap())));}
    use std::os::macos::fs::MetadataExt as MacMetadataExt;
    let expected=ManifestEntry {p:String::new(),s:0,kind:"dir".into(),hash:String::new(),link:None,mode:md.mode(),uid:md.uid(),gid:md.gid(),m:md.mtime(),mn:md.mtime_nsec(),c:md.ctime(),cn:md.ctime_nsec(),dev:md.dev(),ino:md.ino(),flags:md.st_flags(),xattrs:attrs,acl:read_acl(&root).unwrap()};
    let d=fixture();let archive=d.0.join("root.tar.gz");
    let mut cmd=Command::new("/usr/bin/tar");cmd.args(["--format=pax","--acls","--xattrs","--fflags","--no-recursion","-czf"]).arg(&archive).arg("-C").arg(root.parent().unwrap()).arg(root.file_name().unwrap());
    require_success("Root metadata fixture",&run_with_timeout(cmd,std::time::Duration::from_secs(60)).unwrap()).unwrap();
    verify_archive_source(&archive,root.file_name().unwrap().to_str().unwrap(),&[expected]).unwrap();
    println!("ACTUAL_ROOT_METADATA_ROUNDTRIP passed; no child contents read");
}

#[test]
#[ignore = "manual read-only real-source probe; requires BACKUP_PROBE_SOURCE"]
fn actual_source_backup_finalize_and_test_restore() {
    if std::env::var("BACKUP_PROBE_WAIT_FOR_UNLOCK").as_deref() == Ok("1") { crate::protected_access::enable_real_lock_state(); }
    let source=PathBuf::from(std::env::var("BACKUP_PROBE_SOURCE").expect("Explicit source required"));
    let d=fixture();let backup=d.0.join("backup");fs::create_dir(&backup).unwrap();
    let expected=compute_snapshot(&source).unwrap();let bytes=expected.iter().map(|e|e.s).sum();
    let name=archive_name_for(&source,"tar.gz");let archive=backup.join(&name);
    create_verified_archive_from_snapshot(&source,&archive,true,&expected).unwrap();
    let meta=BackupMetadata {timestamp:"20260907-120000".into(),items:vec![BackupItem {path:source.to_str().unwrap().into(),archive:name,hash:hash_file(&archive).unwrap(),archive_size_bytes:fs::metadata(&archive).unwrap().len(),source_size_bytes:bytes}],hash_algorithm:"sha256".into(),total_source_size_bytes:bytes,start_time:String::new(),end_time:String::new(),duration_seconds:0};
    finish_backup(&backup,&meta,&[(source.clone(),expected.clone())]).unwrap();
    let destination=d.0.join("restore");fs::create_dir(&destination).unwrap();
    let result=test_restore_to(&backup,source.to_str().unwrap(),&destination).unwrap();
    let restored=compute_snapshot(&Path::new(&result.extracted_path).join(source.file_name().unwrap())).unwrap();
    assert_eq!(restored.len(),expected.len());
    for (actual,original) in restored.iter().zip(&expected) {assert!(readback_differences(actual,original).is_empty());}
    println!("ACTUAL_SOURCE_BACKUP_AND_RESTORE passed: {} files, {} bytes",result.file_count,result.bytes_extracted);
}

#[test]
fn access_preflight_collects_nested_denials_across_sources() {
    let d = fixture();
    let one = d.0.join("first"); let two = d.0.join("second");
    fs::create_dir_all(one.join("nested")).unwrap(); fs::create_dir(&two).unwrap();
    let blocked = [one.join("nested/unreadable"), two.join("another")];
    for p in &blocked { fs::write(p,b"private").unwrap(); fs::set_permissions(p,fs::Permissions::from_mode(0)).unwrap(); }
    let result = validate_source_access(&[one.to_str().unwrap().into(),two.to_str().unwrap().into()], &d.0);
    for p in &blocked { fs::set_permissions(p,fs::Permissions::from_mode(0o600)).unwrap(); }
    let error = result.unwrap_err();
    for p in &blocked { assert!(error.contains(p.to_str().unwrap()),"{error}"); }
    assert!(error.contains("2 Problem(e)"));
    assert!(error.contains("Datei öffnen"));
}

#[test]
fn access_preflight_preserves_links_and_runtime_socket_policy() {
    use std::os::unix::net::UnixListener;
    let d = fixture(); let source=d.0.join("source");fs::create_dir(&source).unwrap();
    let locked=d.0.join("locked"); fs::write(&locked,b"private").unwrap(); fs::set_permissions(&locked,fs::Permissions::from_mode(0)).unwrap();
    symlink(&locked,source.join("link")).unwrap();symlink("missing",source.join("dangling")).unwrap();
    let socket=source.join("s");let _listener=UnixListener::bind(&socket).unwrap();
    let result=validate_source_access(&[source.to_str().unwrap().into()], &d.0);
    fs::set_permissions(&locked,fs::Permissions::from_mode(0o600)).unwrap();
    result.unwrap();
    assert!(validate_source_access(&[socket.to_str().unwrap().into()],&d.0).is_err());
}

#[test]
fn source_replacement_during_read_restarts_hash_from_zero() {
    let d=fixture();let source=d.0.join("file");let replacement=d.0.join("replacement");
    fs::write(&source, vec![b'a'; 2*1024*1024]).unwrap();fs::write(&replacement,b"current-version").unwrap();
    let mut replaced=false;
    let snapshot=scan_with_activity(&source,&mut |a| {
        if a.bytes>0 && !replaced {fs::rename(&replacement,&source).unwrap(); replaced=true;}
    }).unwrap();
    assert!(replaced);assert_eq!(snapshot,compute_snapshot(&source).unwrap());
    assert_eq!(snapshot[0].hash,format!("{:x}",Sha256::digest(b"current-version")));
    assert_eq!(snapshot[0].s,15);
}

#[test]
fn continuous_replacement_exhausts_bounded_retries() {
    let d=fixture();let source=d.0.join("file");fs::write(&source,b"initial").unwrap();
    let mut replacements=0;
    let error=scan_with_activity(&source,&mut |a| {
        if a.bytes>0 && !a.boundary {
            let p=d.0.join("replacement");fs::write(&p,b"updated").unwrap();fs::rename(&p,&source).unwrap();replacements+=1;
        }
    }).unwrap_err();
    assert_eq!(replacements,3);assert!(error.contains("3 Versuche"));
}

#[test]
fn entry_retries_never_retry_permission_errors_and_honor_cancel() {
    let d=fixture();let path=d.0.join("denied");let mut attempts=0;
    let result:Result<(),String>=retry_entry(&path,|| { attempts+=1;Err(EntryReadError::Other(access_error(&path,"Dateiinhalt lesen",std::io::Error::from_raw_os_error(libc::EPERM)))) });
    assert_eq!(attempts,1);assert!(result.unwrap_err().contains("Dateiinhalt lesen"));
    attempts=0;
    let result:Result<(),String>=retry_entry(&path,|| { attempts+=1;BACKUP_CANCELLED.store(true,Ordering::SeqCst);Err(EntryReadError::Changed) });
    BACKUP_CANCELLED.store(false,Ordering::SeqCst);
    assert_eq!(attempts,1);assert!(result.unwrap_err().contains("abgebrochen"));
}

#[test]
fn access_preflight_can_be_cancelled() {
    let d=fixture();BACKUP_CANCELLED.store(true,Ordering::SeqCst);
    let result=validate_source_access(&[d.0.to_str().unwrap().into()],&d.0);
    BACKUP_CANCELLED.store(false,Ordering::SeqCst);assert!(result.unwrap_err().contains("abgebrochen"));
}

#[test]
#[ignore = "manual access-only probe; requires BACKUP_PROBE_CONFIG"]
fn actual_selected_sources_access_preflight() {
    let config:BackupConfig=serde_json::from_slice(&fs::read(std::env::var("BACKUP_PROBE_CONFIG").unwrap()).unwrap()).unwrap();
    validate_source_access(&config.directories,&dirs::home_dir().unwrap()).unwrap();
    println!("ALL_SELECTED_SOURCES_ACCESS_PREFLIGHT passed");
}

#[test]
fn quarantine_survives_roundtrip_with_os_regenerated_value() {
    let d=fixture();let source=d.0.join("file");fs::write(&source,b"payload").unwrap();
    xattr::set(&source,"com.apple.quarantine",b"0086;6a9f2328;com.apple.cfprefsd;").unwrap();
    let expected=compute_snapshot(&source).unwrap();let archive=d.0.join("file.tar.gz");
    create_verified_archive_from_snapshot(&source,&archive,true,&expected).unwrap();
    let output=d.0.join("restored");fs::create_dir(&output).unwrap();unpack_private(&archive,&output).unwrap();
    let actual=compute_snapshot(&output.join("file")).unwrap();
    assert!(actual[0].xattrs.contains_key("com.apple.quarantine"));
    assert!(readback_differences(&actual[0],&expected[0]).is_empty());
    let mut missing=actual[0].clone();missing.xattrs.remove("com.apple.quarantine");
    assert!(readback_differences(&missing,&expected[0]).contains(&"Erweitertes Attribut com.apple.quarantine".into()));
    let mut source_changed=expected[0].clone();source_changed.xattrs.insert("com.apple.quarantine".into(),"changed".into());
    assert_ne!(expected[0],source_changed);
}

#[test]
fn final_source_check_identifies_changed_added_and_removed_paths() {
    let d=fixture();let source=d.0.join("source");fs::create_dir(&source).unwrap();
    fs::write(source.join("changed"),b"old").unwrap();fs::write(source.join("removed"),b"gone").unwrap();
    let expected=compute_snapshot(&source).unwrap();
    fs::write(source.join("changed"),b"new").unwrap();fs::remove_file(source.join("removed")).unwrap();fs::write(source.join("added"),b"new").unwrap();
    let error=ensure_unchanged(&source,&expected).unwrap_err();
    assert!(error.contains("changed: Dateiinhalt (SHA-256)"));
    assert!(error.contains("removed: entfernt"));assert!(error.contains("added: neu hinzugekommen"));
}


#[test]
fn tracked_document_roundtrips_with_hidden_flag_content_and_metadata() {
    let d=fixture();let source=d.0.join("document");fs::write(&source,b"document data").unwrap();
    xattr::set(&source,"com.example.backup",b"metadata").unwrap();
    let path=CString::new(source.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe {libc::chflags(path.as_ptr(),libc::UF_TRACKED | libc::UF_HIDDEN)},0);
    let expected=compute_snapshot(&source).unwrap();
    assert_eq!(expected[0].flags,libc::UF_TRACKED | libc::UF_HIDDEN);
    let archive=d.0.join("document.tar.gz");create_verified_archive_from_snapshot(&source,&archive,true,&expected).unwrap();
    let output=d.0.join("restored");fs::create_dir(&output).unwrap();unpack_private(&archive,&output).unwrap();
    let actual=compute_snapshot(&output.join("document")).unwrap();
    assert_eq!(actual[0].flags,expected[0].flags);
    assert_eq!(actual[0].hash,expected[0].hash);assert_eq!(actual[0].xattrs.get("com.example.backup"),expected[0].xattrs.get("com.example.backup"));
    assert!(readback_differences(&actual[0],&expected[0]).is_empty());
    ensure_unchanged(&source,&expected).unwrap();
    assert_eq!(unsafe {libc::chflags(path.as_ptr(),libc::UF_HIDDEN)},0);
    assert!(ensure_unchanged(&source,&expected).unwrap_err().contains("Dateiflags"));
}

#[test]
fn readback_rejects_every_changed_flag_bit() {
    let d=fixture();let source=d.0.join("file");fs::write(&source,b"data").unwrap();
    let expected=compute_snapshot(&source).unwrap().remove(0);
    for bit in 0..32 {
        let mut actual=expected.clone();actual.flags ^= 1u32 << bit;
        assert!(readback_differences(&actual,&expected).iter().any(|d|d.contains("Dateiflags")));
    }
}

#[test]
fn compressed_file_roundtrip_retains_content_and_compression_flag() {
    let d=fixture();let plain=d.0.join("plain");let compressed=d.0.join("compressed");fs::write(&plain,vec![b'x';128*1024]).unwrap();
    let output=Command::new("/usr/bin/ditto").arg("--hfsCompression").arg(&plain).arg(&compressed).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let expected=compute_snapshot(&compressed).unwrap();assert_ne!(expected[0].flags & libc::UF_COMPRESSED,0);
    let archive=d.0.join("compressed.tar.gz");create_verified_archive_from_snapshot(&compressed,&archive,true,&expected).unwrap();
    verify_archive_source(&archive,"compressed",&expected).unwrap();
    assert_eq!(fs::read(&compressed).unwrap(),fs::read(&plain).unwrap());
}

#[test]
fn tracked_flags_survive_directory_merge_hardlinks_and_symlinks_without_live_source() {
    let d=fixture();let source=d.0.join("tree");fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/document"),b"tracked bytes").unwrap();
    fs::hard_link(source.join("nested/document"),source.join("hardlink")).unwrap();
    let outside=d.0.join("outside");fs::write(&outside,b"untouched").unwrap();
    symlink(&outside,source.join("link")).unwrap();
    for rel in ["","nested","nested/document","link"] {
        let path=if rel.is_empty(){source.clone()}else{source.join(rel)};
        crate::archive_flags::set(&path,libc::UF_TRACKED | libc::UF_HIDDEN).unwrap();
    }
    let baseline=compute_snapshot(&source).unwrap();
    let archive=d.0.join("tree.tar.gz");create_verified_archive_from_snapshot(&source,&archive,true,&baseline).unwrap();
    fs::remove_dir_all(&source).unwrap();
    let target=d.0.join("restore/tree");fs::create_dir_all(target.join("nested")).unwrap();
    staged_restore(&archive,&target,true).unwrap();
    let restored=compute_snapshot(&target).unwrap();
    for (actual,expected) in restored.iter().zip(&baseline) {assert!(readback_differences(actual,expected).is_empty(),"{:?}",readback_differences(actual,expected));}
    assert_eq!(restored.len(),baseline.len());
    assert_eq!(fs::metadata(target.join("hardlink")).unwrap().ino(),fs::metadata(target.join("nested/document")).unwrap().ino());
    assert_eq!(compute_snapshot(&outside).unwrap()[0].flags,0);
}

#[test]
fn internal_flags_filename_never_replaces_user_data() {
    let d=fixture();
    for name in [".macos-backup-suite-flags-v1",".macos-backup-suite-flags-v1-alternate",".MACOS-BACKUP-SUITE-FLAGS-V1"] {
        let source=d.0.join(name);
        fs::write(&source,b"macOS Backup Suite file flags v1\n{arbitrary user data}").unwrap();
        crate::archive_flags::set(&source,libc::UF_TRACKED).unwrap();
        let archive=d.0.join("archive");create_verified_archive(&source,&archive,true).unwrap();
        let output=PrivateDir::temp().unwrap();unpack_private(&archive,&output.0).unwrap();
        assert_eq!(fs::read(output.0.join(name)).unwrap(),fs::read(&source).unwrap());
        assert_eq!(fs::read_dir(&output.0).unwrap().count(),1);
        fs::remove_file(&source).unwrap();
    }
}

#[test]
fn invalid_flags_metadata_is_rejected_before_extracting_files() {
    let d=fixture();
    for entries in [
        vec![("../outside",libc::UF_TRACKED)],
        vec![("/outside",libc::UF_TRACKED)],
        vec![("missing",libc::UF_TRACKED)],
        vec![("",libc::UF_TRACKED),("",libc::UF_HIDDEN)],
    ] {
        let metadata=crate::archive_flags::write(&d.0,&crate::archive_flags::Flags {root:"file".into(),pax_metadata:false,entries:entries.into_iter().map(|(p,flags)|crate::archive_flags::Record{path:p.into(),flags}).collect()}).unwrap();
        let source=d.0.join("file");fs::write(&source,b"payload").unwrap();
        let archive=d.0.join("invalid.tar.gz");
        let output=Command::new("/usr/bin/tar").current_dir(&d.0).args(["--format=pax","-czf"]).arg(&archive).arg("./file").arg(format!("@{}",metadata.display())).output().unwrap();
        assert!(output.status.success());
        let target=PrivateDir::temp().unwrap();
        assert!(unpack_private(&archive,&target.0).is_err());
        assert!(fs::read_dir(&target.0).unwrap().next().is_none());
    }
}

#[test]
fn missing_compression_state_is_never_faked_by_setting_its_flag() {
    let d=fixture();let source=d.0.join("plain");fs::write(&source,b"plain data").unwrap();
    assert!(crate::archive_flags::set(&source,libc::UF_COMPRESSED).unwrap_err().contains("Systemflags"));
    assert_eq!(fs::read(&source).unwrap(),b"plain data");
    assert_eq!(compute_snapshot(&source).unwrap()[0].flags,0);
}

#[test]
fn immutable_files_and_directories_restore_with_their_protection_flags() {
    let owned=ReadbackDir(fixture());let d=&owned.0;
    let source=d.0.join("locked");fs::create_dir(&source).unwrap();
    let file=source.join("document");fs::write(&file,b"protected data").unwrap();
    crate::archive_flags::set(&file,libc::UF_IMMUTABLE | libc::UF_TRACKED).unwrap();
    crate::archive_flags::set(&source,libc::UF_IMMUTABLE | libc::UF_HIDDEN).unwrap();
    let expected=compute_snapshot(&source).unwrap();
    let archive=d.0.join("locked.tar.gz");create_verified_archive_from_snapshot(&source,&archive,true,&expected).unwrap();
    for existing in [false,true] {
        let target=d.0.join(if existing {"merged/locked"}else{"fresh/locked"});
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        if existing {fs::create_dir(&target).unwrap();}
        staged_restore(&archive,&target,true).unwrap();
        let restored=compute_snapshot(&target).unwrap();
        assert_eq!(restored.len(),expected.len());
        for (a,b) in restored.iter().zip(&expected) {assert!(readback_differences(a,b).is_empty(),"{:?}",readback_differences(a,b));}
    }
}

#[test]
fn incremental_reuse_ignores_only_the_snapshot_mount_device_number() {
    let d=fixture();let source=d.0.join("file");fs::write(&source,b"data").unwrap();
    let a=compute_snapshot(&source).unwrap();let mut b=a.clone();b[0].dev+=1;
    assert!(same_source_version(&a,&b));assert_ne!(a,b);
    b[0].ino+=1;assert!(!same_source_version(&a,&b));b[0].ino-=1;
    b[0].xattrs.insert("com.apple.lastuseddate#PS".into(),"changed".into());assert!(!same_source_version(&a,&b));
}

#[test]
#[ignore = "manual APFS integration; creates a purgeable Time Machine snapshot"]
fn frozen_backup_survives_live_content_and_usage_attribute_changes() {
    let d=fixture();let source=d.0.join("document");fs::write(&source,b"original content").unwrap();
    xattr::set(&source,"com.apple.lastuseddate#PS",&[1u8;16]).unwrap();
    let frozen=crate::frozen_sources::FrozenSources::capture(&[source.clone()]).unwrap();
    let stable=frozen.get(&source).unwrap();let baseline=compute_snapshot(&stable).unwrap();
    // Reproduce opening/editing a document while its backup is still running.
    fs::write(&source,b"edited while backing up").unwrap();
    xattr::set(&source,"com.apple.lastuseddate#PS",&[2u8;16]).unwrap();
    xattr::set(&source,"com.apple.quarantine",b"0086;6a9f2328;com.apple.cfprefsd;").unwrap();
    assert!(fs::OpenOptions::new().write(true).open(&stable).is_err());
    let archive=d.0.join("document.tar.gz");create_verified_archive_from_snapshot(&stable,&archive,true,&baseline).unwrap();
    let output=PrivateDir::temp().unwrap();unpack_private(&archive,&output.0).unwrap();
    let restored=compute_snapshot(&output.0.join("document")).unwrap();
    assert!(readback_differences(&restored[0],&baseline[0]).is_empty());
    assert_eq!(fs::read(output.0.join("document")).unwrap(),b"original content");
    assert_eq!(fs::read(&source).unwrap(),b"edited while backing up");
    assert!(!restored[0].xattrs.contains_key("com.apple.quarantine"));
    drop(frozen);assert!(!stable.exists(),"snapshot view did not unmount");
    println!("FROZEN_BACKUP_LIVE_CHANGES passed: content and both usage attributes preserved at snapshot time");
}

#[test]
#[ignore = "manual real-source APFS backup + finalize + restore; requires BACKUP_PROBE_SOURCE"]
fn actual_frozen_source_backup_finalize_and_test_restore() {
    let source=PathBuf::from(std::env::var("BACKUP_PROBE_SOURCE").expect("Explicit source required"));
    let frozen=crate::frozen_sources::FrozenSources::capture(&[source.clone()]).unwrap();
    let stable=frozen.get(&source).unwrap();
    let d=fixture();let backup=d.0.join("backup");fs::create_dir(&backup).unwrap();
    println!("SNAPSHOT_READY {}",frozen.snapshot);
    let expected=compute_snapshot(&stable).unwrap();let bytes=expected.iter().map(|e|e.s).sum();
    println!("SOURCE_SCANNED {} entries, {} bytes",expected.len(),bytes);
    let name=archive_name_for(&source,"tar.gz");let archive=backup.join(&name);
    create_verified_archive_from_snapshot(&stable,&archive,true,&expected).unwrap();
    println!("ARCHIVE_CREATED_AND_READBACK_VERIFIED");
    let meta=BackupMetadata {timestamp:"20260908-140000".into(),items:vec![BackupItem {path:source.to_str().unwrap().into(),archive:name,hash:hash_file(&archive).unwrap(),archive_size_bytes:fs::metadata(&archive).unwrap().len(),source_size_bytes:bytes}],hash_algorithm:"sha256".into(),total_source_size_bytes:bytes,start_time:String::new(),end_time:String::new(),duration_seconds:0};
    finish_backup(&backup,&meta,&[(stable.clone(),expected.clone())]).unwrap();
    let destination=d.0.join("restore");fs::create_dir(&destination).unwrap();
    let result=test_restore_to(&backup,source.to_str().unwrap(),&destination).unwrap();
    let restored=compute_snapshot(&Path::new(&result.extracted_path).join(source.file_name().unwrap())).unwrap();
    assert_eq!(restored.len(),expected.len());
    for (a,b) in restored.iter().zip(&expected) {assert!(readback_differences(a,b).is_empty(),"{}: {:?}",b.p,readback_differences(a,b));}
    drop(frozen);assert!(!stable.exists());
    println!("ACTUAL_FROZEN_BACKUP_AND_RESTORE passed: {} files, {} bytes",result.file_count,result.bytes_extracted);
}

#[test]
fn literal_appledouble_names_and_resource_forks_survive_backup_and_merge() {
    let d=fixture();let source=d.0.join("tree");fs::create_dir_all(source.join("._directory")).unwrap();
    for (name,bytes) in [("image",b"image data".as_slice()),("._image",b"independent sidecar"),("._orphan",b"no sibling"),("._directory/file",b"nested data")] {
        fs::write(source.join(name),bytes).unwrap();
        xattr::set(source.join(name),"com.example.independent",name.as_bytes()).unwrap();
    }
    xattr::set(source.join("image"),"com.apple.ResourceFork",b"resource fork must not replace sidecar").unwrap();
    xattr::set(source.join("._image"),"com.apple.ResourceFork",b"sidecar has its own resource fork").unwrap();
    fs::hard_link(source.join("._image"),source.join("hardlink")).unwrap();
    symlink("._image",source.join("._symlink")).unwrap();
    crate::archive_flags::set(&source.join("._image"),libc::UF_TRACKED | libc::UF_HIDDEN).unwrap();
    let result=Command::new("/bin/chmod").args(["+a","everyone allow read,readattr,readextattr,readsecurity"]).arg(source.join("._image")).output().unwrap();
    assert!(result.status.success());
    let expected=compute_snapshot(&source).unwrap();let archive=d.0.join("tree.tar.gz");
    create_verified_archive_from_snapshot(&source,&archive,true,&expected).unwrap();
    fs::remove_dir_all(&source).unwrap();
    let target=d.0.join("restored/tree");fs::create_dir_all(&target).unwrap();
    staged_restore(&archive,&target,true).unwrap();
    let actual=compute_snapshot(&target).unwrap();assert_eq!(actual.len(),expected.len());
    for (a,b) in actual.iter().zip(&expected) {assert!(readback_differences(a,b).is_empty(),"{} {:?}",b.p,readback_differences(a,b));}
    assert_eq!(fs::metadata(target.join("hardlink")).unwrap().ino(),fs::metadata(target.join("._image")).unwrap().ino());
}

#[test]
fn literal_appledouble_root_and_metadata_companion_are_user_files() {
    let d=fixture();
    for name in ["._document","._.macos-backup-suite-flags-v1","._.macos-backup-suite-flags-v1-alternate"] {
        let source=d.0.join(name);fs::write(&source,b"literal user file").unwrap();
        let expected=compute_snapshot(&source).unwrap();let archive=d.0.join("archive");
        create_verified_archive_from_snapshot(&source,&archive,true,&expected).unwrap();
        let target=d.0.join("out").join(name);fs::create_dir_all(target.parent().unwrap()).unwrap();
        staged_restore(&archive,&target,true).unwrap();
        assert_eq!(fs::read(&target).unwrap(),b"literal user file");
    }
}

#[test]
fn legacy_appledouble_archive_retains_native_metadata_restore() {
    let d=fixture();let source=d.0.join("legacy");fs::write(&source,b"legacy bytes").unwrap();
    xattr::set(&source,"com.apple.ResourceFork",b"legacy resource fork").unwrap();
    let baseline=compute_snapshot(&source).unwrap();let archive=d.0.join("legacy.tar.gz");
    let metadata=crate::archive_flags::write(&d.0,&crate::archive_flags::Flags{root:"legacy".into(),pax_metadata:false,entries:Vec::new()}).unwrap();
    let result=Command::new("/usr/bin/tar").current_dir(&d.0).args(["--format=pax","-czf"]).arg(&archive).arg("./legacy").arg(format!("@{}",metadata.display())).output().unwrap();
    assert!(result.status.success());verify_archive_source(&archive,"legacy",&baseline).unwrap();
    let old:crate::archive_flags::Flags=serde_json::from_str(r#"{"root":"legacy","entries":[]}"#).unwrap();assert!(!old.pax_metadata);
}

#[test]
fn missing_readback_entries_report_exact_paths() {
    let d=fixture();let source=d.0.join("tree");fs::create_dir(&source).unwrap();fs::write(source.join("present"),b"data").unwrap();
    let archive=d.0.join("archive");create_verified_archive(&source,&archive,true).unwrap();
    fs::write(source.join("._missing"),b"missing").unwrap();
    let error=verify_archive_source(&archive,"tree",&compute_snapshot(&source).unwrap()).unwrap_err();
    assert!(error.contains("._missing"));assert!(error.contains("fehlend"));
}

#[test]
fn resume_reuses_only_unchanged_sources_and_verified_archives() {
    let d=fixture();let source=d.0.join("source");fs::write(&source,b"original").unwrap();
    let backup=d.0.join("backup");let inventory=d.0.join("inventory");fs::create_dir(&backup).unwrap();
    let name="source.tar.gz";let archive=backup.join(name);let initial=compute_snapshot(&source).unwrap();
    create_verified_archive_from_snapshot(&source,&archive,true,&initial).unwrap();
    save_manifest(&inventory,name,&initial).unwrap();
    let item=BackupItem{path:"source".into(),archive:name.into(),hash:hash_file(&archive).unwrap(),archive_size_bytes:fs::metadata(&archive).unwrap().len(),source_size_bytes:8};
    let items=vec![item];let mut current=initial.clone();current[0].dev+=1;
    assert!(verified_resume_item(&backup,&inventory,"source",name,&current,&items).unwrap().is_some());
    current[0].xattrs.insert("com.example.changed".into(),"value".into());
    assert!(verified_resume_item(&backup,&inventory,"source",name,&current,&items).unwrap().is_none());
    fs::write(&source,b"modified").unwrap();set_times(&source,initial[0].m,initial[0].mn);
    assert!(verified_resume_item(&backup,&inventory,"source",name,&compute_snapshot(&source).unwrap(),&items).unwrap().is_none());
    let mut bytes=fs::read(&archive).unwrap();bytes[20]^=1;fs::write(&archive,bytes).unwrap();
    assert!(verified_resume_item(&backup,&inventory,"source",name,&initial,&items).unwrap().is_none());
    fs::remove_file(manifest_path_for(&inventory,name)).unwrap();
    assert!(resume_candidate(&inventory,"source",name,&initial,&items).is_none());
}

#[test]
#[ignore = "manual configured source list; requires BACKUP_PROBE_SOURCES_JSON"]
fn remaining_frozen_sources_backup_and_restore() {
    let sources:Vec<PathBuf>=serde_json::from_slice(&fs::read(std::env::var("BACKUP_PROBE_SOURCES_JSON").unwrap()).unwrap()).unwrap();
    let frozen=crate::frozen_sources::FrozenSources::capture(&sources).unwrap();
    let d=fixture();let backup=d.0.join("backup");fs::create_dir(&backup).unwrap();let mut items=Vec::new();let mut guards=Vec::new();
    for source in &sources {
        let stable=frozen.get(source).unwrap();let expected=compute_snapshot(&stable).unwrap();
        println!("PROBE_SOURCE {} entries={} bytes={}",source.display(),expected.len(),expected.iter().map(|e|e.s).sum::<u64>());
        let name=archive_name_for(source,"tar.zst");let archive=backup.join(&name);
        create_verified_archive_from_snapshot(&stable,&archive,false,&expected).unwrap();
        items.push(BackupItem{path:source.to_str().unwrap().into(),archive:name,hash:hash_file(&archive).unwrap(),archive_size_bytes:fs::metadata(&archive).unwrap().len(),source_size_bytes:expected.iter().map(|e|e.s).sum()});
        guards.push((stable,expected));println!("PROBE_READBACK_PASSED {}",source.display());
    }
    let meta=BackupMetadata{timestamp:"20260908-182000".into(),total_source_size_bytes:items.iter().map(|e|e.source_size_bytes).sum(),items,hash_algorithm:"sha256".into(),start_time:String::new(),end_time:String::new(),duration_seconds:0};
    finish_backup(&backup,&meta,&guards).unwrap();
    let output=d.0.join("restore");fs::create_dir(&output).unwrap();
    for (source,(_,expected)) in sources.iter().zip(&guards) {
        let restored=test_restore_to(&backup,source.to_str().unwrap(),&output).unwrap();
        let actual=compute_snapshot(&Path::new(&restored.extracted_path).join(source.file_name().unwrap())).unwrap();
        assert_eq!(actual.len(),expected.len());
        for(a,b)in actual.iter().zip(expected) {assert!(readback_differences(a,b).is_empty(),"{} {:?}",b.p,readback_differences(a,b));}
        println!("PROBE_RESTORE_PASSED {}",source.display());
    }
    println!("ALL_REMAINING_SOURCES_PASSED {}",sources.len());
}
