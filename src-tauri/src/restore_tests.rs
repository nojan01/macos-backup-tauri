use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Fixture {
    _dir: PrivateDir,
    root: PathBuf,
    backup: PathBuf,
    home: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = PrivateDir::temp().unwrap();
        let root = dir.0.canonicalize().unwrap();
        let backup = root.join("backup");
        let home = root.join("home");
        fs::create_dir(&backup).unwrap();
        fs::create_dir(&home).unwrap();
        Self {
            _dir: dir,
            root,
            backup,
            home,
        }
    }
    fn metadata(&self, items: Vec<BackupItem>) {
        let m = BackupMetadata {
            timestamp: "20260907-120000".into(),
            items,
            hash_algorithm: "sha256".into(),
            total_source_size_bytes: 1,
            start_time: "".into(),
            end_time: "".into(),
            duration_seconds: 0,
        };
        fs::write(
            self.backup.join("metadata.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
    }
    fn archive(&self, source: &Path, item: &str) -> BackupItem {
        let name = archive_name_for(source, "tar.gz");
        let a = self.backup.join(&name);
        let f = fs::File::create(&a).unwrap();
        let gz = GzEncoder::new(f, Compression::default());
        let mut tar = tar::Builder::new(gz);
        tar.follow_symlinks(false);
        if fs::symlink_metadata(source).unwrap().is_dir() {
            tar.append_dir_all(source.file_name().unwrap(), source)
                .unwrap();
        } else {
            tar.append_path_with_name(source, source.file_name().unwrap())
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        BackupItem {
            path: item.into(),
            archive: name,
            hash: hash_file(&a).unwrap(),
            archive_size_bytes: fs::metadata(&a).unwrap().len(),
            source_size_bytes: 1,
        }
    }
    fn file(&self, path: &str, content: &[u8]) -> PathBuf {
        let p = self.root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
        p
    }
    fn restore(&self, items: &[&str], overwrite: bool) -> Result<RestoreResult, String> {
        restore_selected(
            &self.backup,
            &items.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            overwrite,
            &self.home,
            None,
        )
    }
}
#[test]
fn directory_roundtrip_preserves_bytes_links_and_modes() {
    let f = Fixture::new();
    f.file("source/Documents/Grüße.txt", b"data\0\xff");
    let root = f.root.join("source/Documents");
    f.file("source/Documents/plain", b"target");
    symlink("plain", root.join("link")).unwrap();
    symlink("absent", root.join("dangling")).unwrap();
    fs::set_permissions(root.join("plain"), fs::Permissions::from_mode(0o600)).unwrap();
    let item = f.archive(&root, "~/Documents");
    f.metadata(vec![item]);
    assert_eq!(
        f.restore(&["~/Documents"], false).unwrap().restored_count,
        1
    );
    assert_eq!(
        fs::read(f.home.join("Documents/Grüße.txt")).unwrap(),
        b"data\0\xff"
    );
    assert_eq!(
        fs::read_link(f.home.join("Documents/link")).unwrap(),
        PathBuf::from("plain")
    );
    assert_eq!(
        fs::read_link(f.home.join("Documents/dangling")).unwrap(),
        PathBuf::from("absent")
    );
    assert_eq!(
        fs::metadata(f.home.join("Documents/plain"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[test]
fn single_file_overwrite_and_skip() {
    let f = Fixture::new();
    let src = f.file("source/.gitconfig", b"backup");
    let item = f.archive(&src, "~/.gitconfig");
    f.metadata(vec![item]);
    f.file("home/.gitconfig", b"live");
    assert_eq!(
        f.restore(&["~/.gitconfig"], false).unwrap().skipped_count,
        1
    );
    assert_eq!(fs::read(f.home.join(".gitconfig")).unwrap(), b"live");
    assert_eq!(
        f.restore(&["~/.gitconfig"], true).unwrap().restored_count,
        1
    );
    assert_eq!(fs::read(f.home.join(".gitconfig")).unwrap(), b"backup");
}
#[test]
fn merge_adds_missing_files_and_keeps_existing() {
    let f = Fixture::new();
    f.file("source/Documents/missing", b"new");
    f.file("source/Documents/existing", b"backup");
    f.file("home/Documents/existing", b"live");
    let item = f.archive(&f.root.join("source/Documents"), "~/Documents");
    f.metadata(vec![item]);
    let r = f.restore(&["~/Documents"], false).unwrap();
    assert_eq!(r.error_count, 0);
    assert_eq!(
        fs::read(f.home.join("Documents/existing")).unwrap(),
        b"live"
    );
    assert_eq!(fs::read(f.home.join("Documents/missing")).unwrap(), b"new");
}
#[test]
fn corrupted_archive_never_creates_live_target() {
    let f = Fixture::new();
    let a = f.file("broken.tar.gz", b"broken");
    assert!(staged_restore(&a, &f.home.join("Documents"), true).is_err());
    assert!(!f.home.join("Documents").exists());
}
#[test]
fn wrong_hash_blocks_normal_and_test_restore() {
    let f = Fixture::new();
    let src = f.file("source/.gitconfig", b"data");
    let mut item = f.archive(&src, "~/.gitconfig");
    item.hash = "0".repeat(64);
    f.metadata(vec![item]);
    assert!(f.restore(&["~/.gitconfig"], true).is_err());
    assert!(test_restore_to(&f.backup, "~/.gitconfig", &f.home).is_err());
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
}
#[test]
fn preflight_checks_every_item_before_any_write() {
    let f = Fixture::new();
    let one = f.archive(&f.file("source/one", b"one"), "~/one");
    let mut two = f.archive(&f.file("source/two", b"two"), "~/two");
    two.hash = "0".repeat(64);
    f.metadata(vec![one, two]);
    assert!(f.restore(&["~/one", "~/two"], true).is_err());
    assert!(!f.home.join("one").exists());
}
#[test]
fn wrong_archive_root_does_not_overwrite_sibling() {
    let f = Fixture::new();
    f.file("source/Desktop/note", b"backup");
    f.file("home/Desktop/note", b"live");
    let item = f.archive(&f.root.join("source/Desktop"), "~/Documents");
    f.metadata(vec![item]);
    assert!(f.restore(&["~/Documents"], true).is_err());
    assert_eq!(fs::read(f.home.join("Desktop/note")).unwrap(), b"live");
}
#[test]
fn distinct_sources_get_distinct_archives() {
    let f = Fixture::new();
    f.file("first/Documents/note", b"first");
    f.file("second/Documents/note", b"second");
    let a = f.archive(&f.root.join("first/Documents"), "~/Documents");
    let hash = a.hash.clone();
    let b = f.archive(&f.root.join("second/Documents"), "/Users/other/Documents");
    assert_ne!(a.archive, b.archive);
    assert_eq!(hash_file(&f.backup.join(&a.archive)).unwrap(), hash);
    assert_ne!(
        archive_name_for(Path::new("/a/Foo Bar"), "tar.gz"),
        archive_name_for(Path::new("/a/foo-bar"), "tar.gz")
    );
}
#[test]
fn ambiguous_legacy_metadata_is_rejected() {
    let f = Fixture::new();
    let a = f.archive(&f.file("source/a", b"a"), "~/a");
    let mut b = a.clone();
    b.path = "~/b".into();
    f.metadata(vec![a, b]);
    assert!(load_backup_metadata(&f.backup.join("metadata.json")).is_err());
}
#[test]
fn invalid_hashes_and_algorithm_are_rejected() {
    let f = Fixture::new();
    let mut a = f.archive(&f.file("source/a", b"a"), "~/a");
    a.hash.clear();
    f.metadata(vec![a]);
    assert!(load_backup_metadata(&f.backup.join("metadata.json")).is_err());
    let path = f.backup.join("metadata.json");
    let mut json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    json["items"][0]["hash"] = serde_json::json!("f".repeat(64));
    json["hash_algorithm"] = serde_json::json!("md5");
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(load_backup_metadata(&path).is_err());
}
#[test]
fn archive_failure_is_not_reported_as_success() {
    let f = Fixture::new();
    let archive = f.root.join(if is_zstd_available() {
        "bad.tar.zst"
    } else {
        "bad.tar.gz"
    });
    assert!(create_tar_gz(&f.root.join("missing-source"), &archive).is_err());
    assert!(!archive.exists());
}
#[test]
fn system_tar_archive_roundtrip() {
    let f = Fixture::new();
    f.file("source/Ordner mit Umlaut ä/note", b"data");
    let source = f.root.join("source/Ordner mit Umlaut ä");
    let a = f.root.join(if is_zstd_available() {
        "archive.tar.zst"
    } else {
        "archive.tar.gz"
    });
    create_tar_gz(&source, &a).unwrap();
    staged_restore(&a, &f.home.join("Ordner mit Umlaut ä"), false).unwrap();
    assert_eq!(
        fs::read(f.home.join("Ordner mit Umlaut ä/note")).unwrap(),
        b"data"
    );
}
#[test]
fn safari_roundtrip_restores_all_mappings_and_honors_overwrite() {
    let f = Fixture::new();
    f.file("source/safari_backup/Bookmarks.plist", b"bookmarks");
    f.file(
        "source/safari_backup/com.apple.Safari.plist",
        b"preferences",
    );
    f.file("source/safari_backup/Favicon Cache/icon", b"icon");
    let item = f.archive(&f.root.join("source/safari_backup"), "safari-settings");
    f.metadata(vec![item]);
    f.file("home/Library/Safari/Bookmarks.plist", b"live");
    assert_eq!(
        f.restore(&["safari-settings"], false).unwrap().error_count,
        0
    );
    assert_eq!(
        fs::read(f.home.join("Library/Safari/Bookmarks.plist")).unwrap(),
        b"live"
    );
    assert_eq!(
        fs::read(f.home.join("Library/Preferences/com.apple.Safari.plist")).unwrap(),
        b"preferences"
    );
    assert!(f.home.join("Library/Safari/Favicon Cache/icon").exists());
    assert_eq!(
        f.restore(&["safari-settings"], true).unwrap().error_count,
        0
    );
    assert_eq!(
        fs::read(f.home.join("Library/Safari/Bookmarks.plist")).unwrap(),
        b"bookmarks"
    );
}
#[test]
fn cache_restores_at_correct_depth_for_both_legacy_roots() {
    for root in ["Homebrew", "cache"] {
        let f = Fixture::new();
        f.file(&format!("source/{root}/downloads/package"), b"cache");
        let item = f.archive(&f.root.join(format!("source/{root}")), "homebrew-cache");
        f.metadata(vec![item]);
        assert_eq!(
            f.restore(&["homebrew-cache"], false).unwrap().error_count,
            0
        );
        assert_eq!(
            fs::read(f.home.join("Library/Caches/Homebrew/downloads/package")).unwrap(),
            b"cache"
        );
    }
}
#[test]
fn destination_symlinks_cannot_redirect_directory_merge() {
    let f = Fixture::new();
    f.file("source/Documents/note", b"backup");
    f.file("outside/note", b"live");
    symlink(f.root.join("outside"), f.home.join("Documents")).unwrap();
    let item = f.archive(&f.root.join("source/Documents"), "~/Documents");
    f.metadata(vec![item]);
    assert_eq!(f.restore(&["~/Documents"], true).unwrap().error_count, 1);
    assert_eq!(fs::read(f.root.join("outside/note")).unwrap(), b"live");
}
#[test]
fn leaf_symlinks_are_replaced_without_following_them() {
    let f = Fixture::new();
    let item = f.archive(&f.file("source/note", b"backup"), "~/note");
    f.metadata(vec![item]);
    let outside = f.file("outside", b"live");
    symlink(&outside, f.home.join("note")).unwrap();
    assert_eq!(f.restore(&["~/note"], true).unwrap().error_count, 0);
    assert_eq!(fs::read(&outside).unwrap(), b"live");
    assert_eq!(fs::read(f.home.join("note")).unwrap(), b"backup");
}
#[test]
fn test_restore_uses_unique_private_folders() {
    let f = Fixture::new();
    let item = f.archive(&f.file("source/note", b"backup"), "~/note");
    f.metadata(vec![item]);
    let a = test_restore_to(&f.backup, "~/note", &f.home).unwrap();
    let b = test_restore_to(&f.backup, "~/note", &f.home).unwrap();
    assert_ne!(a.extracted_path, b.extracted_path);
    assert_eq!(a.file_count, 1);
    assert!(!f.home.join("note").exists());
    assert!(test_restore_to(&f.backup, "~/note", &f.backup).is_err());
}
#[test]
fn malformed_metadata_does_not_get_verified_label() {
    let f = Fixture::new();
    let backup = f.root.join("macos-backup-suite/data/test");
    fs::create_dir_all(&backup).unwrap();
    fs::write(backup.join("metadata.json"), b"invalid").unwrap();
    let result = list_backups(f.root.to_string_lossy().into()).unwrap();
    assert!(!result[0].hash_verified);
    assert!(!result[0].metadata_valid);
}
#[test]
fn extension_injection_is_rejected_before_installation() {
    assert!(extension_ids("ms-python.python\nms-vscode.cpptools").is_ok());
    for input in [
        "foo.bar; touch /tmp/pwn",
        "foo.bar $(id)",
        "--force",
        "foo/bar.vsix",
        "foo.bar\"",
        "foo.bar\ninvalid",
    ] {
        assert!(extension_ids(input).is_err(), "{input}");
    }
}
#[test]
fn brew_inventory_is_data_not_ruby() {
    assert_eq!(
        brew_entries(
            "tap \"homebrew/core\"\nbrew \"node@22\", restart_service: :changed\ncask \"iterm2\"\n"
        )
        .unwrap()
        .len(),
        3
    );
    for input in [
        "system('id')",
        "brew \"#{system('id')}\"",
        "brew \"node\"; system('id')",
        "brew \"--help\"",
    ] {
        assert!(brew_entries(input).is_err(), "{input}");
    }
}
#[test]
fn every_nonzero_process_status_is_an_error() {
    let output = std::process::Output {
        status: {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(127 << 8)
        },
        stdout: b"Installing one\n".to_vec(),
        stderr: b"brew: command not found".to_vec(),
    };
    assert!(require_success("brew", &output).is_err());
}
#[test]
fn partial_extension_failure_is_an_error() {
    let f = Fixture::new();
    let script = f.file(
        "fake-code",
        b"#!/bin/sh\ncase \"$2\" in good.extension) exit 0;; *) exit 1;; esac\n",
    );
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let r = install_extensions(
        script.to_str().unwrap(),
        &["good.extension".into(), "bad.extension".into()],
        false,
    );
    assert!(r.unwrap_err().contains("1/2"));
}
#[test]
fn timestamp_traversal_is_rejected() {
    for value in ["", ".", "..", "../../outside", "/absolute", "a\\b"] {
        assert!(validate_component(value).is_err());
    }
}
fn crafted_archive(f: &Fixture, entries: &[(&str, u8, &str)]) -> PathBuf {
    let p = f.root.join("crafted.tar.gz");
    let gz = GzEncoder::new(fs::File::create(&p).unwrap(), Compression::default());
    let mut tar = tar::Builder::new(gz);
    for (name, kind, link) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o600);
        h.set_size(0);
        h.set_entry_type(tar::EntryType::new(*kind));
        // Set raw bytes so unsafe paths can exercise our validator, not the builder's.
        h.as_mut_bytes()[..100].fill(0);
        h.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        if !link.is_empty() {
            h.set_link_name(link).unwrap();
        }
        h.set_cksum();
        tar.append(&h, std::io::empty()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    p
}
#[test]
fn archive_traversal_and_special_files_are_rejected() {
    for name in ["../outside", "/absolute", "Documents/../../outside"] {
        let f = Fixture::new();
        let a = crafted_archive(&f, &[(name, b'0', "")]);
        assert!(archive_index(&a).is_err());
    }
    let f = Fixture::new();
    let a = crafted_archive(&f, &[("Documents/fifo", b'6', "")]);
    assert!(archive_index(&a).is_err());
}
#[test]
fn archive_symlink_ancestors_and_external_hardlinks_are_rejected() {
    let f = Fixture::new();
    let a = crafted_archive(
        &f,
        &[
            ("Documents/link", b'2', "/tmp"),
            ("Documents/link/file", b'0', ""),
        ],
    );
    assert!(archive_index(&a).is_err());
    let a = crafted_archive(&f, &[("Documents/file", b'1', "../outside")]);
    assert!(archive_index(&a).is_err());
}
#[test]
fn mismatched_extension_is_detected_by_magic() {
    let f = Fixture::new();
    let item = f.archive(&f.file("source/note", b"bytes"), "~/note");
    let a = f.root.join("incorrect.tar.zst");
    fs::copy(f.backup.join(item.archive), &a).unwrap();
    staged_restore(&a, &f.home.join("note"), false).unwrap();
    assert_eq!(fs::read(f.home.join("note")).unwrap(), b"bytes");
}
#[test]
fn backup_valid_metadata_is_not_automatically_verified() {
    let f = Fixture::new();
    let item = f.archive(&f.file("source/note", b"data"), "~/note");
    f.metadata(vec![item]);
    let path = f.root.join("macos-backup-suite/data/test");
    fs::create_dir_all(&path).unwrap();
    fs::copy(f.backup.join("metadata.json"), path.join("metadata.json")).unwrap();
    let r = list_backups(f.root.to_string_lossy().into()).unwrap();
    assert!(r[0].metadata_valid);
    assert!(!r[0].hash_verified);
}
#[test]
fn missing_archive_blocks_all_selected_items() {
    let f = Fixture::new();
    let one = f.archive(&f.file("source/one", b"one"), "~/one");
    let two = f.archive(&f.file("source/two", b"two"), "~/two");
    fs::remove_file(f.backup.join(&two.archive)).unwrap();
    f.metadata(vec![one, two]);
    assert!(f.restore(&["~/one", "~/two"], false).is_err());
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
}
#[test]
fn unreadable_or_missing_safari_payload_is_an_error() {
    let f = Fixture::new();
    f.file("source/safari_backup/unrelated", b"data");
    let item = f.archive(&f.root.join("source/safari_backup"), "safari-settings");
    f.metadata(vec![item]);
    assert_eq!(
        f.restore(&["safari-settings"], false).unwrap().error_count,
        1
    );
}
#[test]
fn safari_root_symlink_is_never_followed() {
    let f = Fixture::new();
    let outside = f.file("outside/Bookmarks.plist", b"outside");
    fs::create_dir(f.root.join("source")).unwrap();
    symlink(
        outside.parent().unwrap(),
        f.root.join("source/safari_backup"),
    )
    .unwrap();
    let item = f.archive(&f.root.join("source/safari_backup"), "safari-settings");
    f.metadata(vec![item]);
    assert_eq!(
        f.restore(&["safari-settings"], true).unwrap().error_count,
        1
    );
    assert!(!f.home.join("Library/Safari/Bookmarks.plist").exists());
}
#[test]
fn truncated_gzip_is_rejected_even_with_matching_hash() {
    let f = Fixture::new();
    let mut item = f.archive(&f.file("source/note", b"backup"), "~/note");
    let a = f.backup.join(&item.archive);
    let file = fs::OpenOptions::new().write(true).open(&a).unwrap();
    file.set_len(item.archive_size_bytes - 4).unwrap();
    item.archive_size_bytes -= 4;
    item.hash = hash_file(&a).unwrap();
    f.metadata(vec![item]);
    assert!(f.restore(&["~/note"], true).is_err());
    assert!(!f.home.join("note").exists());
}
#[test]
fn source_directory_is_not_replaced_by_conflicting_file() {
    let f = Fixture::new();
    let item = f.archive(&f.file("source/note", b"backup"), "~/note");
    f.metadata(vec![item]);
    f.file("home/note/keep", b"live");
    assert_eq!(f.restore(&["~/note"], true).unwrap().error_count, 1);
    assert_eq!(fs::read(f.home.join("note/keep")).unwrap(), b"live");
}
#[test]
fn brew_nonzero_status_propagates_after_partial_success() {
    let f = Fixture::new();
    let script=f.file("fake-brew",b"#!/bin/sh\nif [ \"$1\" = list ]; then exit 1; fi\nif [ \"$3\" = good ]; then exit 0; fi\necho 'command not found' >&2\nexit 127\n");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let entries = brew_entries("brew \"good\"\nbrew \"bad\"").unwrap();
    let result = install_brew_entries(script.to_str().unwrap(), &entries, false, None).unwrap_err();
    assert!(result.contains("1 Homebrew entries completed"));
    assert!(result.contains("command not found"));
}
#[test]
fn normal_restore_cannot_write_into_backup() {
    let f = Fixture::new();
    let item = f.archive(
        &f.file("source/note", b"backup"),
        f.backup.join("note").to_str().unwrap(),
    );
    let path = item.path.clone();
    f.metadata(vec![item]);
    let result = f.restore(&[&path], true).unwrap();
    assert_eq!(result.error_count, 1);
    assert!(!f.backup.join("note").exists());
}
#[test]
fn rebuilding_a_hardlinked_archive_does_not_modify_previous_backup() {
    let f = Fixture::new();
    f.file("source/Documents/note", b"first");
    let source = f.root.join("source/Documents");
    let previous = f.root.join("previous.tar");
    let resumed = f.root.join("resumed.tar");
    create_tar_gz(&source, &previous).unwrap();
    let hash = hash_file(&previous).unwrap();
    fs::hard_link(&previous, &resumed).unwrap();
    f.file("source/Documents/note", b"second");
    create_tar_gz(&source, &resumed).unwrap();
    assert_eq!(hash_file(&previous).unwrap(), hash);
    assert_ne!(hash_file(&resumed).unwrap(), hash);
}
#[test]
fn failed_archive_rebuild_keeps_previous_archive() {
    let f = Fixture::new();
    f.file("source/Documents/note", b"first");
    let source = f.root.join("source/Documents");
    let archive = f.root.join("archive.tar");
    create_tar_gz(&source, &archive).unwrap();
    let hash = hash_file(&archive).unwrap();
    assert!(create_tar_gz(&f.root.join("missing"), &archive).is_err());
    assert_eq!(hash_file(&archive).unwrap(), hash);
}
#[test]
fn production_single_file_archive_roundtrip() {
    let f = Fixture::new();
    let source = f.file("source/.gitconfig", b"[user]\nname=Test\n");
    let archive = f.root.join("single.tar.gz");
    create_file_archive(&source, ".gitconfig", &archive).unwrap();
    staged_restore(&archive, &f.home.join(".gitconfig"), false).unwrap();
    assert_eq!(
        fs::read(f.home.join(".gitconfig")).unwrap(),
        fs::read(source).unwrap()
    );
}
#[test]
fn only_one_operation_can_run_at_a_time() {
    let guard = OperationGuard::acquire().unwrap();
    assert!(OperationGuard::acquire().is_err());
    assert!(ensure_operation_idle().is_err());
    drop(guard);
    assert!(OperationGuard::acquire().is_ok());
}
#[test]
fn case_collisions_and_case_aliased_link_ancestors_are_rejected() {
    let f = Fixture::new();
    let a = crafted_archive(
        &f,
        &[("Documents/Note", b'0', ""), ("Documents/note", b'0', "")],
    );
    assert!(archive_index(&a).is_err());
    let a = crafted_archive(
        &f,
        &[
            ("Documents/Link", b'2', "/tmp"),
            ("Documents/link/file", b'0', ""),
        ],
    );
    assert!(archive_index(&a).is_err());
}
#[test]
fn verbose_processes_do_not_deadlock_on_full_pipes() {
    let f = Fixture::new();
    let script=f.file("verbose",b"#!/bin/sh\ni=0\nwhile [ $i -lt 10000 ]; do printf 'stdout message\\n'; printf 'stderr message\\n' >&2; i=$((i+1)); done\n");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let output =
        run_with_timeout(Command::new(&script), std::time::Duration::from_secs(10)).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 150000);
    assert_eq!(output.stderr.len(), 150000);
}
#[test]
fn timeout_also_cleans_up_children_holding_output_pipes() {
    let f = Fixture::new();
    let script = f.file("child", b"#!/bin/sh\nsleep 10 &\nexit 0\n");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let start = std::time::Instant::now();
    assert!(
        run_with_timeout(Command::new(&script), std::time::Duration::from_millis(200)).is_err()
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
}
