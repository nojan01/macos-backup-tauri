//! Optional settings are discovered afresh. Explicit user sources stay strict.
use crate::BackupConfig;
use serde::Serialize;
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
pub(crate) struct SettingsSources {
    pub id: &'static str,
    pub name: &'static str,
    pub paths: Vec<String>,
}

// The current desktop app shares configuration between ChatGPT and Codex.
const SHARED: &[&str] = &[
    ".codex/config.toml",
    ".codex/.codex-global-state.json",
    "Library/Preferences/com.openai.codex.plist",
    "Library/Application Support/com.openai.codex",
];
const VSCODE: &[&str] = &[
    "Library/Application Support/Code/User",
    ".vscode/argv.json",
    "Library/Preferences/com.microsoft.VSCode.plist",
];
const CHATGPT: &[&str] = &[
    "Library/Preferences/com.openai.chat.plist",
    "Library/Preferences/com.openai.chatgpt.plist",
    "Library/Application Support/com.openai.chat",
];
const CODEX: &[&str] = &[
    ".codex/AGENTS.md",
    ".codex/hooks.json",
    ".codex/rules",
    ".agents/skills",
    "Library/Application Support/Codex/Local State",
    "Library/Application Support/Codex/Default/Preferences",
    "Library/Application Support/Codex/Default/Secure Preferences",
    "Library/Application Support/Codex/browser-sidebar-page-states.json",
    "Library/Application Support/OpenAI/Codex/NativeMessagingHosts",
];

fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("App-Einstellungen {}: {e}", path.display())),
    }
}
fn entries(path: &Path) -> Result<Vec<PathBuf>, String> {
    if !exists(path)? {
        return Ok(Vec::new());
    }
    let mut result = fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .map(|entry| {
            entry
                .map(|e| e.path())
                .map_err(|e| format!("{}: {e}", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    result.sort();
    Ok(result)
}

pub(crate) fn discover(home: &Path, config: &BackupConfig) -> Result<Vec<SettingsSources>, String> {
    let mut groups = Vec::new();
    for (id, name, enabled, candidates) in [
        ("vscode", "VS Code", config.backup_vscode_settings, VSCODE),
        (
            "chatgpt",
            "ChatGPT",
            config.backup_chatgpt_settings,
            CHATGPT,
        ),
        ("codex", "Codex", config.backup_codex_settings, CODEX),
    ] {
        if !enabled {
            continue;
        }
        let mut paths = Vec::new();
        for rel in candidates.iter().chain(if id == "vscode" {
            [].iter()
        } else {
            SHARED.iter()
        }) {
            if exists(&home.join(rel))? {
                paths.push(format!("~/{rel}"));
            }
        }
        if id == "codex" {
            // Custom profiles and custom skills; bundled skills and plugin binaries
            // are reinstallable. Never pull in sessions, auth, caches or worktrees.
            for path in entries(&home.join(".codex"))? {
                if path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.ends_with(".config.toml"))
                {
                    paths.push(format!(
                        "~/{}",
                        path.strip_prefix(home)
                            .unwrap()
                            .to_str()
                            .ok_or("Ungültiger Konfigurationspfad")?
                    ));
                }
            }
            for path in entries(&home.join(".codex/skills"))? {
                if path.file_name().is_some_and(|s| s != ".system") {
                    paths.push(format!(
                        "~/{}",
                        path.strip_prefix(home)
                            .unwrap()
                            .to_str()
                            .ok_or("Ungültiger Skill-Pfad")?
                    ));
                }
            }
        }
        groups.push(SettingsSources { id, name, paths });
    }
    Ok(groups)
}

fn expand(source: &str, home: &Path) -> PathBuf {
    if source == "~" {
        home.to_path_buf()
    } else if let Some(rel) = source.strip_prefix("~/") {
        home.join(rel)
    } else {
        PathBuf::from(source)
    }
}

/// Append optional sources only when not already covered by a selected directory.
/// A selected symlink only backs up the link itself, so it cannot cover descendants.
pub(crate) fn append_sources(
    sources: &mut Vec<String>,
    groups: &[SettingsSources],
    home: &Path,
) -> Vec<String> {
    let mut added = Vec::new();
    for group in groups {
        for source in &group.paths {
            let path = expand(source, home);
            if sources.iter().any(|existing| {
                let root = expand(existing, home);
                root == path
                    || (path.starts_with(&root)
                        && fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir()))
            }) {
                continue;
            }
            sources.push(source.clone());
            added.push(source.clone());
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backup::*,
        restore::{unpack_private, PrivateDir},
        BACKUP_CANCELLED, VERIFY_CANCELLED,
    };
    use std::{
        os::unix::fs::{symlink, PermissionsExt},
        sync::atomic::Ordering,
    };
    fn fixture() -> PrivateDir {
        BACKUP_CANCELLED.store(false, Ordering::SeqCst);
        VERIFY_CANCELLED.store(false, Ordering::SeqCst);
        PrivateDir::temp().unwrap()
    }
    fn put(home: &Path, rel: &str, data: &[u8]) {
        let path = home.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
    #[test]
    fn old_configs_enable_optional_settings_and_explicit_false_survives_reload() {
        let old = r#"{"target_volume":"","target_directory":"","directories":[],"backup_homebrew":false,"backup_mas":false}"#;
        let mut config: BackupConfig = serde_json::from_str(old).unwrap();
        assert!(
            config.backup_vscode_settings
                && config.backup_chatgpt_settings
                && config.backup_codex_settings
        );
        config.backup_vscode_settings = false;
        config.backup_chatgpt_settings = false;
        config.backup_codex_settings = false;
        let loaded: BackupConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert!(
            !loaded.backup_vscode_settings
                && !loaded.backup_chatgpt_settings
                && !loaded.backup_codex_settings
        );
    }
    #[test]
    fn absent_apps_are_optional_and_new_settings_are_discovered_next_time() {
        let home = fixture();
        let config = BackupConfig::default();
        assert!(discover(&home.0, &config)
            .unwrap()
            .iter()
            .all(|g| g.paths.is_empty()));
        put(
            &home.0,
            "Library/Application Support/Code/User/settings.json",
            b"{}",
        );
        assert_eq!(
            discover(&home.0, &config).unwrap()[0].paths,
            vec!["~/Library/Application Support/Code/User"]
        );
        fs::remove_dir_all(home.0.join("Library/Application Support/Code")).unwrap();
        assert!(discover(&home.0, &config).unwrap()[0].paths.is_empty());
        // Explicit stale selections still fail the normal source preflight.
        assert!(validate_selected_sources(
            &["~/Library/Application Support/Code/User".into()],
            &home.0.join("out"),
            &home.0
        )
        .is_err());
    }
    #[test]
    fn disabled_options_add_nothing_and_never_remove_manual_sources() {
        let home = fixture();
        put(&home.0, ".codex/config.toml", b"model='test'");
        let config = BackupConfig {
            backup_vscode_settings: false,
            backup_chatgpt_settings: false,
            backup_codex_settings: false,
            ..BackupConfig::default()
        };
        let mut sources = vec!["~/.codex".into()];
        assert!(
            append_sources(&mut sources, &discover(&home.0, &config).unwrap(), &home.0).is_empty()
        );
        assert_eq!(sources, vec!["~/.codex"]);
    }
    #[test]
    fn shared_settings_and_absolute_manual_ancestors_are_not_added_twice() {
        let home = fixture();
        put(&home.0, ".codex/config.toml", b"model='test'");
        put(
            &home.0,
            "Library/Preferences/com.openai.codex.plist",
            b"test",
        );
        let groups = discover(&home.0, &BackupConfig::default()).unwrap();
        let mut sources = vec![home.0.join("Library/Preferences").to_str().unwrap().into()];
        assert_eq!(
            append_sources(&mut sources, &groups, &home.0),
            vec!["~/.codex/config.toml"]
        );
        assert_eq!(sources.len(), 2);
    }
    #[test]
    fn own_skills_and_profiles_are_included_without_runtime_data() {
        let home = fixture();
        for rel in [
            ".codex/skills/mine/SKILL.md",
            ".codex/skills/.system/builtin/SKILL.md",
            ".codex/worktrees/repo/data",
            ".codex/sessions/chat.json",
            ".codex/auth.json",
            ".codex/work.config.toml",
        ] {
            put(&home.0, rel, b"data");
        }
        let groups = discover(&home.0, &BackupConfig::default()).unwrap();
        let paths = &groups.iter().find(|g| g.id == "codex").unwrap().paths;
        assert_eq!(
            paths,
            &vec!["~/.codex/work.config.toml", "~/.codex/skills/mine"]
        );
    }
    #[test]
    fn unreadable_settings_are_errors_not_absent_apps() {
        let home = fixture();
        put(&home.0, ".codex/config.toml", b"data");
        let dir = home.0.join(".codex");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0)).unwrap();
        let result = discover(&home.0, &BackupConfig::default());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
    }
    #[test]
    fn symlink_parent_does_not_hide_child_settings() {
        let home = fixture();
        put(&home.0, "actual/config.toml", b"data");
        symlink("actual", home.0.join(".codex")).unwrap();
        let mut sources = vec!["~/.codex".into()];
        assert_eq!(
            append_sources(
                &mut sources,
                &discover(&home.0, &BackupConfig::default()).unwrap(),
                &home.0
            ),
            vec!["~/.codex/config.toml"]
        );
    }
    #[test]
    fn discovered_settings_roundtrip_through_verified_archives_and_restore() {
        let home = fixture();
        for rel in [
            "Library/Application Support/Code/User/settings.json",
            "Library/Application Support/com.openai.chat/preferences.json",
            ".codex/config.toml",
            ".codex/skills/mine/SKILL.md",
        ] {
            put(&home.0, rel, b"fixture settings\n");
        }
        let mut sources = Vec::new();
        append_sources(
            &mut sources,
            &discover(&home.0, &BackupConfig::default()).unwrap(),
            &home.0,
        );
        assert_eq!(sources.len(), 4);
        for (i, source) in sources.iter().enumerate() {
            let path = expand(source, &home.0);
            let archive = home.0.join(format!("{i}.tar.gz"));
            create_verified_archive(&path, &archive, true).unwrap();
            let stage = PrivateDir::temp().unwrap();
            unpack_private(&archive, &stage.0).unwrap();
            let mut expected = serde_json::to_value(compute_snapshot(&path).unwrap()).unwrap();
            let mut restored = serde_json::to_value(
                compute_snapshot(&stage.0.join(path.file_name().unwrap())).unwrap(),
            )
            .unwrap();
            // A restored file has a new inode and change time; compare every
            // restorable content/metadata field, not live filesystem identity.
            for manifest in [&mut expected, &mut restored] {
                for entry in manifest.as_array_mut().unwrap() {
                    for key in ["c", "cn", "dev", "ino"] {
                        entry.as_object_mut().unwrap().remove(key);
                    }
                }
            }
            assert_eq!(expected, restored);
        }
    }
}
