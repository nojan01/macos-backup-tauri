mod backup;
use backup::*;
mod restore;
use restore::*;

use tauri::Emitter;
use tauri::menu::{Menu, MenuItem, Submenu, PredefinedMenuItem, AboutMetadata};
use tauri::{Manager, AppHandle};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use sha2::{Sha256, Digest};
#[cfg(test)]
use flate2::write::GzEncoder;
#[cfg(test)]
use flate2::Compression;
use walkdir::WalkDir;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;

static BACKUP_CANCELLED: AtomicBool = AtomicBool::new(false);
static VERIFY_CANCELLED: AtomicBool = AtomicBool::new(false);
static TAR_PID: AtomicU32 = AtomicU32::new(0);

/// Cached zstd path - computed once at first use
static ZSTD_PATH: OnceLock<Option<String>> = OnceLock::new();

/// Find and cache the zstd binary path
fn get_zstd_path() -> Option<&'static str> {
    ZSTD_PATH.get_or_init(|| {
        let candidates = [
            "/opt/homebrew/bin/zstd",  // Apple Silicon
            "/usr/local/bin/zstd",      // Intel Mac
        ];
        for candidate in candidates {
            if Path::new(candidate).exists() {
                return Some(candidate.to_string());
            }
        }
        // Fallback: which zstd
        if let Ok(output) = Command::new("/usr/bin/which").arg("zstd").output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(path);
                }
            }
        }
        None
    }).as_deref()
}

/// Check if zstd is available (cached)
fn is_zstd_available() -> bool {
    get_zstd_path().is_some()
}

/// Extract a validated archive into an empty private staging directory.
fn extract_archive_to(archive: &Path, target_dir: &Path) -> Result<(), String> {
    unpack_private(archive, target_dir)
}

fn default_language() -> String {
    "de".to_string()
}

fn default_theme() -> String {
    "auto".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BackupConfig {
    pub target_volume: String,
    pub target_directory: String,
    pub directories: Vec<String>,
    pub backup_homebrew: bool,
    pub backup_mas: bool,
    #[serde(default)]
    pub default_directories: Vec<String>,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub backup_homebrew_cache: bool,
    #[serde(default)]
    pub backup_safari_settings: bool,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            target_volume: String::new(),
            target_directory: String::new(),
            directories: vec![
                "~/Documents".to_string(),
                "~/Desktop".to_string(),
            ],
            backup_homebrew: true,
            backup_mas: true,
            default_directories: Vec::new(),
            language: default_language(),
            theme: default_theme(),
            backup_homebrew_cache: false,
            backup_safari_settings: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BackupItem {
    pub path: String,
    pub archive: String,
    pub hash: String,
    pub archive_size_bytes: u64,
    pub source_size_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupMetadata {
    pub timestamp: String,
    pub items: Vec<BackupItem>,
    pub hash_algorithm: String,
    pub total_source_size_bytes: u64,
    pub start_time: String,
    pub end_time: String,
    pub duration_seconds: u64,
}

/// Validate a parsed `BackupMetadata` to reject unsafe values that could
/// escape the intended backup directory or cause unexpected shell behaviour.
fn validate_backup_metadata(meta: &BackupMetadata) -> Result<(), String> {
    if !meta.hash_algorithm.eq_ignore_ascii_case("sha256") { return Err("Unsupported backup hash algorithm".into()); }
    let mut archives = std::collections::HashSet::new();
    let mut paths = std::collections::HashSet::new();
    for item in &meta.items {
        if item.hash.len() != 64 || !item.hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("Invalid SHA-256 for {}", item.path));
        }
        if !archives.insert(item.archive.to_lowercase()) || !paths.insert(&item.path) {
            return Err("Ambiguous backup: duplicate source or archive names; create a new backup".into());
        }
        validate_component(&item.archive)?;
        let archive = &item.archive;
        // Archive filenames must be plain filenames (no path separators, no ..)
        // because they are joined onto the backup directory and passed to tar.
        if archive.is_empty()
            || archive.contains('/')
            || archive.contains('\\')
            || archive == "."
            || archive == ".."
            || archive.contains("..")
        {
            return Err(format!("Unsafe archive name in metadata: {:?}", archive));
        }

        // Source paths: reject traversal segments. Absolute paths and the
        // special `~` prefix remain allowed, but no `..` segments are permitted.
        let path = &item.path;
        if path.is_empty() || path.contains('\0') {
            return Err("Empty source path in metadata".to_string());
        }
        for seg in path.split('/') {
            if seg == ".." {
                return Err(format!("Path traversal detected in metadata path: {:?}", path));
            }
        }
    }
    Ok(())
}

/// Parse + validate a metadata.json file in one step. Use this everywhere
/// instead of calling `serde_json::from_str` directly, so untrusted backups
/// cannot bypass validation.
fn load_backup_metadata(metadata_path: &Path) -> Result<BackupMetadata, String> {
    let content = fs::read_to_string(metadata_path)
        .map_err(|e| format!("Error reading metadata: {}", e))?;
    let meta: BackupMetadata = serde_json::from_str(&content)
        .map_err(|e| format!("Error parsing metadata: {}", e))?;
    validate_backup_metadata(&meta)?;
    Ok(meta)
}

#[derive(Debug, Serialize, Clone)]
pub struct ProgressUpdate {
    pub message: String,
    pub fraction: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub path: String,
    pub available: bool,
    pub writable: bool,
    pub is_internal: bool,
    pub free_space_gb: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BackupListItem {
    pub timestamp: String,
    pub hash_verified: bool,
    pub metadata_valid: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DetectedBackupVolume {
    pub volume_path: String,
    pub volume_name: String,
    pub backup_count: usize,
    pub latest_timestamp: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct VerifyResult {
    pub success: bool,
    pub total_files: usize,
    pub verified_files: usize,
    pub failed_files: Vec<String>,
    pub message: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct BackupFileInfo {
    pub path: String,
    pub archive: String,
    pub archive_size_bytes: u64,
    pub source_size_bytes: u64,
}

#[derive(Debug, Serialize, Clone)]
pub struct BackupDetails {
    pub timestamp: String,
    pub items: Vec<BackupFileInfo>,
    pub total_source_size_bytes: u64,
    pub total_archive_size_bytes: u64,
    pub start_time: String,
    pub end_time: String,
    pub duration_seconds: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserFolder {
    pub name: String,
    pub path: String,
    pub readable: bool,
    pub is_current_user: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PermissionCheckResult {
    pub path: String,
    pub readable: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FullDiskAccessStatus {
    pub has_full_disk_access: bool,
    pub tested_paths: Vec<String>,
    pub inaccessible_paths: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct RestoreResult {
    pub restored_count: usize,
    pub skipped_count: usize,
    pub error_count: usize,
    pub restored: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppLicenseEntry {
    pub app_name: String,
    #[serde(default)]
    pub registered_name: String,
    #[serde(default)]
    pub license_key: String,
    #[serde(default)]
    pub notes: String,
}

fn get_config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".macos_backup_suite").join("config.json")
}

// Get free space in GB for a path
fn get_free_space_gb(path: &Path) -> f64 {
    let output = Command::new("df")
        .args(["-k", &path.to_string_lossy()])
        .output();
    
    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = stdout.lines().nth(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    if let Ok(kb) = parts[3].parse::<u64>() {
                        return (kb as f64) / (1024.0 * 1024.0);
                    }
                }
            }
        }
    }
    0.0
}

// Check if path is Time Machine volume
fn is_time_machine_volume(path: &Path) -> bool {
    let tm_marker1 = path.join(".timemachine");
    let tm_marker2 = path.join("Backups.backupdb");
    let tm_marker3 = path.join(".com.apple.timemachine.supported");
    
    tm_marker1.exists() || tm_marker2.exists() || tm_marker3.exists()
}

// Check if volume is writable
fn is_writable(path: &Path) -> bool {
    let test_file = path.join(".macos_backup_write_test");
    if fs::write(&test_file, "test").is_ok() {
        let _ = fs::remove_file(&test_file);
        true
    } else {
        false
    }
}

// Check if a path is readable
fn check_readable(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    
    if path.is_file() {
        fs::File::open(path).is_ok()
    } else {
        fs::read_dir(path).is_ok()
    }
}

#[tauri::command]
fn load_config() -> Result<BackupConfig, String> {
    let path = get_config_path();
    if !path.exists() {
        return Ok(BackupConfig::default());
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_config(config: BackupConfig) -> Result<(), String> {
    let path = get_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    atomic_write(&path, content.as_bytes())
}

#[tauri::command]
fn get_external_volumes() -> Result<Vec<Volume>, String> {
    let volumes_path = Path::new("/Volumes");
    let mut volumes = Vec::new();
    
    if let Ok(entries) = fs::read_dir(volumes_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Unknown".to_string());
                
                if name == "Macintosh HD" || name == "Macintosh HD - Data" {
                    continue;
                }
                
                if is_time_machine_volume(&path) {
                    continue;
                }
                
                let path_str = path.to_string_lossy().to_string();
                let available = path.exists() && path.read_dir().is_ok();
                let writable = is_writable(&path);
                let free_space_gb = get_free_space_gb(&path);
                
                if !writable {
                    continue;
                }
                
                let is_internal = name.starts_with("com.apple") 
                    || name == "Recovery" 
                    || name == "Preboot"
                    || name == "VM"
                    || name == "Update";
                
                volumes.push(Volume {
                    name,
                    path: path_str,
                    available,
                    writable,
                    is_internal,
                    free_space_gb,
                });
            }
        }
    }
    Ok(volumes)
}

/// Detect if the app is running from (or next to) a backup volume.
/// Checks:
///  1. The volume the executable is on
///  2. All mounted /Volumes for a macos-backup-suite directory
/// Returns the first volume found that contains backups.
#[tauri::command]
fn detect_backup_volume() -> Result<Option<DetectedBackupVolume>, String> {
    // 1. Check the volume this executable is running from
    if let Ok(exe) = std::env::current_exe() {
        let exe_str = exe.to_string_lossy().to_string();
        // If running from /Volumes/XXX/..., extract the volume path
        if exe_str.starts_with("/Volumes/") {
            let parts: Vec<&str> = exe_str.splitn(4, '/').collect();
            // parts: ["", "Volumes", "VolumeName", "rest..."]
            if parts.len() >= 3 {
                let volume_path = format!("/Volumes/{}", parts[2]);
                let suite_root = PathBuf::from(&volume_path).join("macos-backup-suite");
                if suite_root.exists() && suite_root.is_dir() {
                    let data_dir = suite_root.join("data");
                    let mut backup_count = 0;
                    if let Ok(entries) = fs::read_dir(&data_dir) {
                        backup_count = entries.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count();
                    }
                    let latest = fs::read_to_string(suite_root.join("latest.json")).ok()
                        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                        .and_then(|v| v["latest"].as_str().map(|s| s.to_string()));
                    return Ok(Some(DetectedBackupVolume {
                        volume_path: volume_path.clone(),
                        volume_name: parts[2].to_string(),
                        backup_count,
                        latest_timestamp: latest,
                    }));
                }
            }
        }
    }

    // 2. Scan all mounted volumes for a macos-backup-suite directory
    let volumes_path = Path::new("/Volumes");
    if let Ok(entries) = fs::read_dir(volumes_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            let name = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            // Skip system volumes
            if name == "Macintosh HD" || name == "Macintosh HD - Data" { continue; }
            if is_time_machine_volume(&path) { continue; }

            let suite_root = path.join("macos-backup-suite");
            if suite_root.exists() && suite_root.is_dir() {
                let data_dir = suite_root.join("data");
                let mut backup_count = 0;
                if let Ok(entries) = fs::read_dir(&data_dir) {
                    backup_count = entries.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count();
                }
                if backup_count == 0 { continue; }
                let latest = fs::read_to_string(suite_root.join("latest.json")).ok()
                    .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                    .and_then(|v| v["latest"].as_str().map(|s| s.to_string()));
                return Ok(Some(DetectedBackupVolume {
                    volume_path: path.to_string_lossy().to_string(),
                    volume_name: name,
                    backup_count,
                    latest_timestamp: latest,
                }));
            }
        }
    }

    Ok(None)
}

/// List all user folders under /Users/
#[tauri::command]
fn list_user_folders() -> Result<Vec<UserFolder>, String> {
    let users_path = Path::new("/Users");
    let current_user = dirs::home_dir()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default();
    
    let mut user_folders = Vec::new();
    
    if let Ok(entries) = fs::read_dir(users_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                
                // Skip system folders
                if name == "Shared" || name.starts_with('.') || name == "Guest" {
                    continue;
                }
                
                let path_str = path.to_string_lossy().to_string();
                let is_current = name == current_user;
                let readable = check_readable(&path);
                
                user_folders.push(UserFolder {
                    name,
                    path: path_str,
                    readable,
                    is_current_user: is_current,
                });
            }
        }
    }
    
    // Sort: current user first, then alphabetically
    user_folders.sort_by(|a, b| {
        if a.is_current_user && !b.is_current_user {
            std::cmp::Ordering::Less
        } else if !a.is_current_user && b.is_current_user {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });
    
    Ok(user_folders)
}

/// Check read permissions for a given path
#[tauri::command]
fn check_read_permission(path: String) -> Result<PermissionCheckResult, String> {
    let path_buf = PathBuf::from(&path);
    
    // Expand ~ to home directory
    let expanded = if path.starts_with("~/") {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(&path[2..])
    } else if path == "~" {
        dirs::home_dir().unwrap_or_default()
    } else {
        path_buf
    };
    
    if !expanded.exists() {
        return Ok(PermissionCheckResult {
            path,
            readable: false,
            error_message: Some("Path does not exist".to_string()),
        });
    }
    
    let readable = if expanded.is_file() {
        match fs::File::open(&expanded) {
            Ok(_) => true,
            Err(e) => {
                return Ok(PermissionCheckResult {
                    path,
                    readable: false,
                    error_message: Some(format!("No read permission: {}", e)),
                });
            }
        }
    } else {
        match fs::read_dir(&expanded) {
            Ok(_) => true,
            Err(e) => {
                return Ok(PermissionCheckResult {
                    path,
                    readable: false,
                    error_message: Some(format!("Cannot access directory: {}", e)),
                });
            }
        }
    };
    
    Ok(PermissionCheckResult {
        path,
        readable,
        error_message: None,
    })
}

/// Check if Full Disk Access is granted by testing access to TCC.db
#[tauri::command]
fn check_full_disk_access() -> Result<FullDiskAccessStatus, String> {
    // The TCC.db file is the most reliable FDA test - it always exists and requires FDA
    let tcc_db_path = "/Library/Application Support/com.apple.TCC/TCC.db";
    
    let mut test_paths: Vec<String> = vec![tcc_db_path.to_string()];
    let mut inaccessible: Vec<String> = Vec::new();
    
    // Test 1: Try to actually READ from TCC.db - opening is not enough!
    // Without FDA, opening may succeed but reading will fail
    let tcc_path = Path::new(tcc_db_path);
    let tcc_exists = tcc_path.exists();
    
    let can_access_tcc = if tcc_exists {
        // We must try to read, not just open - macOS allows open but blocks read without FDA.
        // 100 Bytes = SQLite-Header; echte Daten, die TCC ohne FDA zuverlässig blockiert.
        match fs::File::open(tcc_path) {
            Ok(mut file) => {
                let mut buffer = [0u8; 100];
                match file.read(&mut buffer) {
                    // „Operation not permitted" → FDA fehlt. Kurzer Read (< 16 Bytes)
                    // auf der echten SQLite-Datei ist verdächtig — TCC.db ist immer
                    // deutlich größer.
                    Ok(n) => n >= 16 && &buffer[0..16] == b"SQLite format 3\0",
                    Err(_) => false,
                }
            }
            Err(_) => {
                false
            }
        }
    } else {
        // If TCC.db does not exist, try the directory
        fs::read_dir("/Library/Application Support/com.apple.TCC").is_ok()
    };
    
    if !can_access_tcc {
        inaccessible.push(tcc_db_path.to_string());
    }
    
    // Test 2: Try to access another user Library folder (if other users exist)
    let current_user = dirs::home_dir()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default();
    
    if let Ok(entries) = fs::read_dir("/Users") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                
                if name != current_user && name != "Shared" && !name.starts_with('.') && name != "Guest" {
                    let library_path = path.join("Library");
                    let library_str = library_path.to_string_lossy().to_string();
                    test_paths.push(library_str.clone());
                    
                    if library_path.exists() && fs::read_dir(&library_path).is_err() {
                        inaccessible.push(library_str);
                    }
                    break;
                }
            }
        }
    }

    // Test 3: FDA-protected paths within the *current* user's Library.
    // These are readable by their owner only when FDA is granted to the
    // running app. This catches the common case where the machine has a
    // single user account and Test 2 above does nothing.
    if let Some(home) = dirs::home_dir() {
        let fda_protected = [
            home.join("Library/Mail"),
            home.join("Library/Safari"),
            home.join("Library/Messages"),
        ];
        for p in fda_protected.iter() {
            if p.exists() {
                let s = p.to_string_lossy().to_string();
                test_paths.push(s.clone());
                if fs::read_dir(p).is_err() {
                    inaccessible.push(s);
                }
            }
        }
    }

    // FDA ist gewährt, sobald TCC.db lesbar ist. TCC.db selbst ist der
    // macOS-interne Gatekeeper für Full Disk Access — ohne FDA blockiert
    // macOS den Read-Syscall. Die zusätzlichen Probes (andere Nutzer-
    // Libraries, ~/Library/Mail etc.) sind nur Diagnose-Info: sie können
    // auch mit gewährtem FDA fehlschlagen (z. B. wenn Mail nie gestartet
    // wurde, Ordner durch FileVault/SIP leer sind oder Permissions-Bits
    // kaputt sind). Wir würden sonst FDA fälschlich als „nicht gewährt"
    // melden, obwohl der autoritative Test erfolgreich war.
    let has_fda = can_access_tcc;

    Ok(FullDiskAccessStatus {
        has_full_disk_access: has_fda,
        tested_paths: test_paths,
        inaccessible_paths: inaccessible,
    })
}

#[tauri::command]
fn open_privacy_settings() -> Result<(), String> {
    Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn restart_app(app_handle: tauri::AppHandle) -> Result<(), String> {
    // Get the current executable path
    let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;

    // Walk up: Contents/MacOS/<exe>  ->  Contents/MacOS  ->  Contents  ->  *.app
    let app_bundle = exe_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .ok_or_else(|| {
            format!(
                "Cannot determine .app bundle from executable path: {}",
                exe_path.display()
            )
        })?;

    // Sanity check: the walked-up path should end in ".app" when running
    // from a proper bundle. If not, fall back to relaunching the executable
    // directly instead of using `open -n`.
    let relaunch_target: PathBuf = if app_bundle
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("app"))
        .unwrap_or(false)
    {
        app_bundle.to_path_buf()
    } else {
        exe_path.clone()
    };

    Command::new("open")
        .arg("-n")
        .arg(&relaunch_target)
        .spawn()
        .map_err(|e| e.to_string())?;

    // Exit the current instance
    app_handle.exit(0);
    Ok(())
}

// Window state management
#[derive(Debug, Serialize, Deserialize, Clone)]
struct WindowState {
    width: u32,
    height: u32,
    x: i32,
    y: i32,
}

fn get_window_state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/tmp"));
    PathBuf::from(home)
        .join("Library/Application Support/com.nojan.macos-backup-suite")
        .join("window_state.json")
}

#[tauri::command]
fn get_window_state() -> Option<WindowState> {
    let path = get_window_state_path();
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(state) = serde_json::from_str::<WindowState>(&content) {
                return Some(state);
            }
        }
    }
    None
}

#[tauri::command]
fn save_window_state(width: u32, height: u32, x: i32, y: i32) -> Result<(), String> {
    let path = get_window_state_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let state = WindowState { width, height, x, y };
    let content = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}

/// Finde den Homebrew-Pfad (wichtig für GUI-Apps ohne korrekte PATH-Variable)
fn find_brew_path() -> Option<String> {
    // Prüfe zuerst die bekannten Homebrew-Installationspfade
    let candidates = [
        "/opt/homebrew/bin/brew",  // Apple Silicon
        "/usr/local/bin/brew",      // Intel Mac
    ];
    
    for candidate in candidates {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    
    // Fallback: which brew (funktioniert nur wenn PATH korrekt ist)
    if let Ok(output) = Command::new("/usr/bin/which")
        .arg("brew")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }
    
    None
}

/// Finde einen Befehl in Homebrew-Pfaden (für mas, etc.)
fn find_homebrew_command(name: &str) -> Option<String> {
    let homebrew_dirs = ["/opt/homebrew/bin", "/usr/local/bin"];
    
    for dir in homebrew_dirs {
        let path = format!("{}/{}", dir, name);
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }
    
    // Fallback
    if let Ok(output) = Command::new("/usr/bin/which")
        .arg(name)
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }
    
    None
}

#[tauri::command]
fn check_homebrew() -> Result<bool, String> {
    Ok(find_brew_path().is_some())
}

#[tauri::command]
fn check_mas() -> Result<bool, String> {
    Ok(find_homebrew_command("mas").is_some())
}

/// Run a command with a wall-clock timeout. On timeout the process is first
/// asked to terminate (SIGTERM), then forcefully killed (SIGKILL) if it does
/// not exit. Returns an error distinguishing spawn/IO failures from timeouts.
fn run_with_timeout(
    cmd: Command,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    run_streamed(cmd, timeout, None, "", "", 1)
}

/// Like `run_with_timeout` but streams stdout+stderr line-by-line to the
/// given window as `event_name` events (prefixed with `log_prefix` if set).
/// `emit_every_n_lines` == 0 or 1 emits every line; values > 1 emit every Nth
/// line (nützlich für sehr gesprächige Kommandos wie `tar -v`).
/// The collected bytes are still returned in `Output` for downstream parsing.
fn run_streamed(
    mut cmd: Command,
    timeout: std::time::Duration,
    window: Option<&tauri::Window>,
    event_name: &str,
    log_prefix: &str,
    emit_every_n_lines: u32,
) -> Result<std::process::Output, String> {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;
    fn read_output<R: Read + Send + 'static>(
        input: R,
        window: Option<tauri::Window>,
        event: String,
        prefix: String,
        stride: u32,
    ) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(input);
            let mut output = Vec::new();
            let mut line = Vec::new();
            let mut count = 0u64;
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                output.extend_from_slice(&line);
                count += 1;
                if count % u64::from(stride.max(1)) == 0 {
                    if let Some(w) = &window {
                        let text = String::from_utf8_lossy(&line);
                        let _ = w.emit(
                            &event,
                            format!("{}{}", prefix, text.trim_end_matches(['\r', '\n'])),
                        );
                    }
                }
            }
            Ok(output)
        })
    }
    // Drain both pipes while the process runs; otherwise verbose installers can
    // fill a pipe and never exit. A separate process group bounds child lifetimes.
    let mut child = cmd
        .process_group(0)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn command: {e}"))?;
    let pid = child.id();
    let stdout = read_output(
        child.stdout.take().ok_or("Missing stdout")?,
        window.cloned(),
        event_name.into(),
        log_prefix.into(),
        emit_every_n_lines,
    );
    let stderr = read_output(
        child.stderr.take().ok_or("Missing stderr")?,
        window.cloned(),
        event_name.into(),
        log_prefix.into(),
        emit_every_n_lines,
    );
    let deadline = std::time::Instant::now() + timeout;
    let mut status = None;
    let failure = loop {
        if BACKUP_CANCELLED.load(Ordering::SeqCst) || VERIFY_CANCELLED.load(Ordering::SeqCst) { break Some("Vorgang abgebrochen".into()); }
        if status.is_none() {
            match child.try_wait() {
                Ok(s) => status = s,
                Err(e) => break Some(format!("Cannot wait for command: {e}")),
            }
        }
        if status.is_some() && stdout.is_finished() && stderr.is_finished() {
            break None;
        }
        if std::time::Instant::now() >= deadline {
            break Some(format!(
                "Command timed out after {:.1}s",
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    if failure.is_some() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    let out = stdout
        .join()
        .map_err(|_| "stdout reader failed")?
        .map_err(|e| e.to_string())?;
    let err = stderr
        .join()
        .map_err(|_| "stderr reader failed")?
        .map_err(|e| e.to_string())?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(std::process::Output {
        status: status.ok_or("Command exit status missing")?,
        stdout: out,
        stderr: err,
    })
}

#[tauri::command]
fn get_brew_packages() -> Result<String, String> {
    let brew_path = find_brew_path()
        .ok_or_else(|| "Homebrew not found. Please install Homebrew: https://brew.sh".to_string())?;

    let mut cmd = Command::new(&brew_path);
    cmd.args(["bundle", "dump", "--file=-"]);
    let output = run_with_timeout(cmd, std::time::Duration::from_secs(120))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
fn get_mas_apps() -> Result<String, String> {
    let mas_path = find_homebrew_command("mas")
        .ok_or_else(|| "mas not found. Install with: brew install mas".to_string())?;

    let mut cmd = Command::new(&mas_path);
    cmd.arg("list");
    let output = run_with_timeout(cmd, std::time::Duration::from_secs(60))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err("mas not available".to_string())
    }
}

#[tauri::command]
fn get_manual_apps() -> Result<Vec<String>, String> {
    // Hole alle Apps aus /Applications
    let apps_dir = PathBuf::from("/Applications");
    let mut all_apps: Vec<String> = Vec::new();
    
    if let Ok(entries) = fs::read_dir(&apps_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "app") {
                if let Some(name) = path.file_stem() {
                    all_apps.push(name.to_string_lossy().to_string());
                }
            }
        }
    }
    
    // Hole Homebrew Cask Apps
    let mut cask_apps: Vec<String> = Vec::new();
    if let Some(brew_path) = find_brew_path() {
        if let Ok(output) = Command::new(&brew_path)
            .args(["list", "--cask"])
            .output()
        {
            let cask_list = String::from_utf8_lossy(&output.stdout);
            for line in cask_list.lines() {
                cask_apps.push(line.trim().to_lowercase());
            }
        }
    }
    
    // Hole MAS Apps
    let mut mas_apps: Vec<String> = Vec::new();
    if let Some(mas_path) = find_homebrew_command("mas") {
        if let Ok(output) = Command::new(&mas_path)
            .arg("list")
            .output()
        {
            let mas_list = String::from_utf8_lossy(&output.stdout);
            for line in mas_list.lines() {
                // Format: "123456  App Name  (1.0)"
                let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
                if parts.len() >= 2 {
                    let name_part = parts[1].trim();
                    if let Some(name) = name_part.split('(').next() {
                        mas_apps.push(name.trim().to_lowercase());
                    }
                }
            }
        }
    }
    
    // Filtere: behalte nur Apps die weder in Cask noch in MAS sind
    let manual_apps: Vec<String> = all_apps
        .into_iter()
        .filter(|app| {
            let app_lower = app.to_lowercase();
            // Prüfe ob App in Cask-Liste ist (oft ähnliche Namen)
            let in_cask = cask_apps.iter().any(|c| {
                app_lower.contains(c) || c.contains(&app_lower) ||
                app_lower.replace(" ", "-") == *c ||
                app_lower.replace(" ", "") == c.replace("-", "")
            });
            // Prüfe ob App in MAS-Liste ist
            let in_mas = mas_apps.iter().any(|m| {
                app_lower == *m || app_lower.contains(m) || m.contains(&app_lower)
            });
            !in_cask && !in_mas
        })
        .collect();
    
    Ok(manual_apps)
}

#[tauri::command]
fn get_vscode_extensions() -> Result<Vec<String>, String> {
    // Prüfe verschiedene VS Code Installationspfade
    let possible_paths = [
        "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        "/usr/local/bin/code",
        "/opt/homebrew/bin/code",
    ];
    
    let code_path = possible_paths.iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|s| s.to_string());
    
    // Alternativ: which code
    let code_cmd = code_path.or_else(|| {
        Command::new("which")
            .arg("code")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    });
    
    let code_cmd = match code_cmd {
        Some(c) => c,
        None => return Err("VS Code not installed".to_string()),
    };
    
    let mut cmd = Command::new(&code_cmd);
    cmd.arg("--list-extensions");
    let output = run_with_timeout(cmd, std::time::Duration::from_secs(30))
        .map_err(|e| format!("Error fetching extensions: {}", e))?;
    
    if !output.status.success() {
        return Err("Could not retrieve VS Code extensions".to_string());
    }
    
    let extensions: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    
    Ok(extensions)
}

fn manifest_path_for(inventory_root: &Path, archive_name: &str) -> PathBuf {
    inventory_root.join("manifests").join(format!("{}.json", archive_name))
}

fn save_manifest(inventory_root: &Path, archive_name: &str, entries: &[ManifestEntry]) -> Result<(), String> {
    let path = manifest_path_for(inventory_root, archive_name);
    fs::create_dir_all(path.parent().ok_or("Missing manifest parent")?).map_err(|e| e.to_string())?;
    atomic_write(&path, &serde_json::to_vec(entries).map_err(|e| e.to_string())?)
}

fn load_manifest(inventory_root: &Path, archive_name: &str) -> Option<Vec<ManifestEntry>> {
    let path = manifest_path_for(inventory_root, archive_name);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

// --- Resume-Support: abgebrochene Backups können fortgesetzt werden ---
//
// Jedes abgeschlossene Archiv wird als JSON-Zeile in `.resume-state.jsonl`
// innerhalb des Backup-Daten-Roots persistiert. Ein Backup gilt als
// unvollständig, solange `metadata.json` fehlt aber `.resume-state.jsonl`
// existiert. Nach erfolgreichem Abschluss wird die State-Datei gelöscht.

fn resume_state_path(backup_root: &Path) -> PathBuf {
    backup_root.join(".resume-state.jsonl")
}

fn append_resume_entry(backup_root: &Path, item: &BackupItem) -> Result<(), String> {
    let mut entries=load_resume_entries(backup_root);
    entries.retain(|old| old.path != item.path);
    entries.push(item.clone());
    let mut bytes=Vec::new();
    for entry in entries { bytes.extend(serde_json::to_vec(&entry).map_err(|e| e.to_string())?); bytes.push(b'\n'); }
    atomic_write(&resume_state_path(backup_root), &bytes)
}

fn load_resume_entries(backup_root: &Path) -> Vec<BackupItem> {
    let path = resume_state_path(backup_root);
    let Ok(content) = fs::read_to_string(&path) else { return Vec::new(); };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<BackupItem>(l).ok())
        .collect()
}

fn clear_resume_state(backup_root: &Path) {
    let _ = fs::remove_file(resume_state_path(backup_root));
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResumableBackup {
    pub timestamp: String,
    pub target_path: String,
    pub completed_items: usize,
    pub completed_size_bytes: u64,
    pub completed_paths: Vec<String>,
}

/// Listet alle unvollständigen Backups (kein `metadata.json`, aber
/// `.resume-state.jsonl` vorhanden) im gegebenen Ziel-Pfad.
#[tauri::command]
fn list_resumable_backups(target_path: String) -> Result<Vec<ResumableBackup>, String> {
    let suite_root = PathBuf::from(&target_path).join("macos-backup-suite");
    let data_root = suite_root.join("data");
    if !data_root.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    let dir = fs::read_dir(&data_root).map_err(|e| e.to_string())?;
    for entry in dir.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if load_backup_metadata(&p.join("metadata.json")).is_ok() {
            continue; // bereits abgeschlossen
        }
        if !resume_state_path(&p).exists() {
            continue;
        }
        let items = load_resume_entries(&p);

        let ts = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let size = items.iter().map(|i| i.archive_size_bytes).sum();
        let paths = items.iter().map(|i| i.path.clone()).collect();
        result.push(ResumableBackup {
            timestamp: ts,
            target_path: target_path.clone(),
            completed_items: items.len(),
            completed_size_bytes: size,
            completed_paths: paths,
        });
    }
    result.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(result)
}

/// Verwirft ein unvollständiges Backup (löscht Daten- und Inventory-Ordner).
/// Abgeschlossene Backups (mit `metadata.json`) werden nicht angetastet.
#[tauri::command]
fn discard_resumable_backup(target_path: String, timestamp: String) -> Result<(), String> {
    validate_component(&timestamp)?;
    let _guard = OperationGuard::acquire()?;
    // Pfad-Traversal-Schutz: Timestamp darf keine Separator enthalten
    if timestamp.contains('/') || timestamp.contains("..") || timestamp.is_empty() {
        return Err("Ungültiger Timestamp".to_string());
    }
    let suite_root = PathBuf::from(&target_path).join("macos-backup-suite");
    let data_dir = suite_root.join("data").join(&timestamp);
    if data_dir.exists() {
        if load_backup_metadata(&data_dir.join("metadata.json")).is_ok() {
            return Err("Backup ist bereits abgeschlossen und kann nicht verworfen werden".to_string());
        }
        fs::remove_dir_all(&data_dir).map_err(|e| format!("Daten-Ordner: {}", e))?;
    }
    let inv_dir = suite_root.join("inventories").join(&timestamp);
    if inv_dir.exists() {
        let _ = fs::remove_dir_all(&inv_dir);
    }
    Ok(())
}

/// Liefert Timestamp + Metadata des jeweils letzten Backups oder None.
fn load_previous_backup(suite_root: &Path) -> Option<(String, BackupMetadata)> {
    let latest = suite_root.join("latest.json");
    let content = fs::read_to_string(&latest).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let ts = v.get("latest")?.as_str()?.to_string();
    validate_component(&ts).ok()?;
    let meta_path = suite_root.join("data").join(&ts).join("metadata.json");
    let meta = load_backup_metadata(&meta_path).ok()?;
    Some((ts, meta))
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    // Larger buffer for less syscalls on multi-GB archives
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut iter: u32 = 0;

    loop {
        // Cancel-Check alle ~4 MiB (jede 4. Iteration) – minimaler Overhead
        iter = iter.wrapping_add(1);
        if iter % 4 == 0
            && (BACKUP_CANCELLED.load(Ordering::Relaxed)
                || VERIFY_CANCELLED.load(Ordering::Relaxed))
        {
            return Err("Vorgang abgebrochen".to_string());
        }
        let bytes_read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    
    Ok(format!("{:x}", hasher.finalize()))
}

fn create_tar_gz(source: &Path, target: &Path) -> Result<(), String> {
    create_verified_archive(source,target,false)
}

fn create_file_archive(source: &Path, entry_name: &str, target: &Path) -> Result<(), String> {
    validate_component(entry_name)?;
    if source.file_name().and_then(|s| s.to_str()) == Some(entry_name) {
        return create_verified_archive(source,target,true);
    }
    let expected=compute_snapshot(source)?;
    let stage=PrivateDir::new(target.parent().ok_or("Missing parent")?,".alias")?;
    let alias=stage.0.join(entry_name);
    let mut cmd=Command::new("/usr/bin/ditto");
    cmd.arg(source).arg(&alias);
    require_success("Copy inventory",&run_with_timeout(cmd,std::time::Duration::from_secs(3600))?)?;
    ensure_unchanged(source,&expected)?;
    create_verified_archive(&alias,target,true)
}

#[tauri::command]
async fn create_backup(
    target_path: String,
    directories: Vec<String>,
    window: tauri::Window,
    incremental: Option<bool>,
    resume_timestamp: Option<String>,
) -> Result<BackupMetadata, String> {
    tauri::async_runtime::spawn_blocking(move || {
        create_backup_impl(target_path, directories, window, incremental, resume_timestamp)
    })
    .await
    .map_err(|e| format!("Backup task join error: {}", e))?
}

fn create_backup_impl(
    target_path: String,
    directories: Vec<String>,
    window: tauri::Window,
    incremental: Option<bool>,
    resume_timestamp: Option<String>,
) -> Result<BackupMetadata, String> {
    let _guard = OperationGuard::acquire()?;
    let _progress = BackupProgress::attach(window.clone());
    // Debug-Trace-Closure (No-op in Release-Builds). Für Diagnose kann hier
    // wieder ein Schreiber in /tmp/macos-backup-trace.log aktiviert werden.
    let trace = |_msg: &str| {};
    trace(&format!(
        "ENTRY target='{}' dirs={} resume={:?}",
        target_path, directories.len(), resume_timestamp
    ));
    for (i, d) in directories.iter().enumerate() {
        trace(&format!("  dir[{}] = {}", i, d));
    }

    let mut seen_sources = std::collections::HashSet::new();
    for dir in &directories {
        if !seen_sources.insert(dir) { return Err(format!("Duplicate backup source: {dir}")); }
    }
    let incremental = incremental.unwrap_or(true);
    let start = Local::now();
    let start_time_str = start.format("%d.%m.%Y %H:%M:%S").to_string();

    // Sofort-Feedback an das UI: ohne diesen Emit sieht der Nutzer sekunden-
    // lang nichts, während der Pre-flight-Scan (compute_directory_size) läuft
    // — das wirkt wie eingefroren / Beachball.
    let emit_r1 = window.emit(
        "backup-progress",
        serde_json::json!({
            "progress": 0,
            "message": "Backup wird vorbereitet..."
        }),
    );
    trace(&format!("first progress emit: {:?}", emit_r1));
    let emit_r2 = window.emit("backup-log", "Starte Backup-Vorbereitung...");
    trace(&format!("first log emit: {:?}", emit_r2));

    let target=Path::new(&target_path);
    if !target.is_absolute() || !fs::metadata(target).map_err(|e|format!("Backup-Ziel nicht erreichbar: {e}"))?.is_dir() {
        return Err("Backup-Ziel muss ein vorhandenes absolutes Verzeichnis sein".into());
    }
    let suite_root = target.join("macos-backup-suite");

    // --- Resume-Modus: bestehenden Backup-Ordner wiederverwenden ---
    let (timestamp, is_resume, resumed_items) = if let Some(ref ts) = resume_timestamp {
        // Pfad-Traversal-Schutz
        if ts.contains('/') || ts.contains("..") || ts.is_empty() {
            return Err("Ungültiger Resume-Timestamp".to_string());
        }
        let candidate = suite_root.join("data").join(ts);
        if !candidate.exists() {
            return Err(format!("Resume-Ziel existiert nicht: {}", ts));
        }
        if load_backup_metadata(&candidate.join("metadata.json")).is_ok() {
            return Err("Dieses Backup ist bereits abgeschlossen".to_string());
        }
        let entries = load_resume_entries(&candidate);
        let _ = window.emit(
            "backup-log",
            format!(
                "♻️  Resume-Modus: {} bereits gesicherte Einträge aus {}",
                entries.len(),
                ts
            ),
        );
        (ts.clone(), true, entries)
    } else {
        (start.format("%Y%m%d-%H%M%S").to_string(), false, Vec::new())
    };

    let backup_root = suite_root.join("data").join(&timestamp);
    let inventory_root = suite_root.join("inventories").join(&timestamp);

    // --- Pre-flight: disk space check ---
    // Estimate total source size and compare against free space on the target
    // volume (with 10% safety margin). Compression usually reduces the on-disk
    // footprint significantly, so this is a conservative upper bound.
    let _ = window.emit(
        "backup-progress",
        serde_json::json!({ "progress": 1, "message": "Scanne Quellverzeichnisse..." }),
    );
    trace("pre-flight scan start");
    // Vorheriges Backup vorab laden – sowohl für die inkrementelle
    // Archiv-Wiederverwendung als auch für eine realistische
    // Speicherplatz-Schätzung: unveränderte Verzeichnisse werden später per
    // Hardlink übernommen und benötigen keinen zusätzlichen Platz.
    let previous = if incremental {
        load_previous_backup(&suite_root)
    } else {
        None
    };
    let home_pre = dirs::home_dir().unwrap_or_default();
    let mut estimated_source_bytes: u64 = 0;
    let mut estimated_new_bytes: u64 = 0;
    let pre_total = directories.len().max(1);

    // Baseline includes content and metadata for every source. It is compared again
    // before archiving and before publication; it never substitutes for a fresh scan.
    let mut cached_snapshots: Vec<Option<Vec<ManifestEntry>>> = vec![None; directories.len()];

    for (pre_i, dir) in directories.iter().enumerate() {
        let expanded = if dir.starts_with("~/") {
            home_pre.join(&dir[2..])
        } else if dir == "~" {
            home_pre.clone()
        } else {
            PathBuf::from(dir)
        };
        trace(&format!("scan[{}/{}] {}", pre_i + 1, pre_total, dir));
        let _ = window.emit(
            "backup-progress",
            serde_json::json!({
                "progress": 1 + (5 * (pre_i + 1) / pre_total),
                "message": format!("Scanne {} ({}/{})", dir, pre_i + 1, pre_total)
            }),
        );
        if !expanded.is_absolute() || expanded.file_name().is_none() || expanded.components().any(|c| matches!(c,std::path::Component::ParentDir)) {
            return Err(format!("Ungültiger Backup-Quellpfad: {dir}"));
        }
        validate_source_target(&expanded, Path::new(&target_path))?;
        let t0 = std::time::Instant::now();
        let snap = compute_snapshot(&expanded)?;
        let added: u64 = snap.iter().map(|e| e.s).sum();
        let snap_opt = Some(snap);

        // Inkrementell: Wird dieses Verzeichnis voraussichtlich unverändert
        // sein (gleiches Manifest wie im Vorgänger, Archiv vorhanden), wird es
        // später per Hardlink übernommen und braucht keinen neuen Speicher.
        // Solche Einträge fließen daher NICHT in die Bedarfsschätzung ein.
        let mut will_reuse = false;
        if let (Some(snap), Some((prev_ts, prev_meta))) = (snap_opt.as_ref(), previous.as_ref()) {
            let archive_ext = if is_zstd_available() { "tar.zst" } else { "tar.gz" };
            let archive_name = archive_name_for(&expanded, archive_ext);
            let prev_inventory = suite_root.join("inventories").join(prev_ts);
            if let Some(prev_snapshot) = load_manifest(&prev_inventory, &archive_name) {
                if &prev_snapshot == snap
                    && prev_meta.items.iter().any(|it| it.path == *dir && it.archive == archive_name)
                    && suite_root.join("data").join(prev_ts).join(&archive_name).exists()
                {
                    will_reuse = true;
                }
            }
        }

        cached_snapshots[pre_i] = snap_opt;
        let dt = t0.elapsed().as_secs_f32();
        trace(&format!("  -> {} bytes in {:.2}s (reuse={})", added, dt, will_reuse));
        estimated_source_bytes = estimated_source_bytes.saturating_add(added);
        if !will_reuse {
            estimated_new_bytes = estimated_new_bytes.saturating_add(added);
        }
    }
    trace(&format!("pre-flight scan done, total {} bytes", estimated_source_bytes));
    let largest_source = cached_snapshots.iter().flatten().map(|s| s.iter().map(|e|e.s).sum::<u64>()).max().unwrap_or(0);
    if estimated_source_bytes > 0 {
        let free_gb = get_free_space_gb(Path::new(&target_path));
        // Reserve room for both archive and extracted readback verification.
        let estimated_gb = (estimated_new_bytes as f64) / (1024.0 * 1024.0 * 1024.0);
        require_free_space(&std::env::temp_dir(),largest_source.saturating_add(largest_source/10))?;
        let same_volume = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&target_path).map_err(|e|e.to_string())?.dev() == fs::metadata(std::env::temp_dir()).map_err(|e|e.to_string())?.dev()
        };
        let required_gb = (estimated_new_bytes as f64 * 1.1 + if same_volume {largest_source as f64 * 1.1} else {0.0}) / (1024.0 * 1024.0 * 1024.0); // 10% margin
        let _ = window.emit(
            "backup-log",
            format!(
                "Free space check: {:.2} GB free, ~{:.2} GB new/changed (need ≥ {:.2} GB with margin)",
                free_gb, estimated_gb, required_gb
            ),
        );
        if free_gb < required_gb {
            let msg = format!(
                "Insufficient free space on target: {:.2} GB free, ~{:.2} GB required (new/changed {:.2} GB plus readback and reserve). Aborting.",
                free_gb, required_gb, estimated_gb
            );
            let _ = window.emit("backup-log", format!("❌ {}", msg));
            let _ = window.emit(
                "backup-progress",
                serde_json::json!({ "progress": 0, "message": "Insufficient disk space" }),
            );
            return Err(msg);
        }
    }

    trace("disk space check done, creating dirs");
    fs::create_dir_all(suite_root.join("data")).map_err(|e| e.to_string())?;
    if !is_resume {
        fs::create_dir(&backup_root).map_err(|e| format!("Cannot create a new backup (timestamp already used?): {e}"))?;
    }
    fs::create_dir_all(&inventory_root).map_err(|e| e.to_string())?;
    atomic_write(&resume_state_path(&backup_root), b"")?;
    trace("dirs created");
    
    let _ = window.emit("backup-log", format!("=== Backup started: {} ===", start_time_str));
    let _ = window.emit("backup-progress", serde_json::json!({
        "progress": 1,
        "message": "Initialisiere Backup..."
    }));
    
    let config = load_config()?;
    let _ = window.emit("backup-log", "Vollständige Sicherung ohne versteckte Ausschlüsse; Rückleseprüfung benötigt zusätzlichen temporären Speicher.");
    let brew_inventory = if config.backup_homebrew { Some(get_brew_packages()?) } else { None };
    let mas_inventory = if config.backup_mas { Some(get_mas_apps()?) } else { None };
    let vscode_inventory = match get_vscode_extensions() {
        Ok(items) => Some(items.join("\n")),
        Err(e) if e=="VS Code not installed" => {let _ = window.emit("backup-log", e); None},
        Err(e) => return Err(e),
    };
    let manual=get_manual_apps()?.join("\n");
    atomic_write(&inventory_root.join("manual_apps.txt"),manual.as_bytes())?;

    let home = dirs::home_dir().unwrap_or_default();
    let mut extra_source_guards: Vec<(PathBuf,Vec<ManifestEntry>)> = Vec::new();
    let mut items = Vec::new();
    let total = directories.len();
    trace(&format!("main loop begin, incremental={} total={}", incremental, total));

    // Resume rebuilds every selected source; previous completed checkpoints are
    // hints only and must never hide changes made since interruption.
    if is_resume { let _ = window.emit("backup-log", format!("Resume: {} frühere Einträge werden anhand der aktuellen Quellen neu geprüft und gesichert", resumed_items.len())); }
    // Inkrementelles Backup: vorheriges Backup ermitteln (Timestamp + Metadata),
    // um Manifeste vergleichen und Archive per Hardlink wiederverwenden zu können.
    trace(&format!("previous backup found={}", previous.is_some()));
    if let Some((prev_ts, _)) = &previous {
        let _ = window.emit(
            "backup-log",
            format!("🔁 Inkrementeller Modus aktiv (Basis: {})", prev_ts),
        );
    } else if incremental {
        let _ = window.emit(
            "backup-log",
            "🔁 Inkrementeller Modus aktiv (kein Basis-Backup gefunden — Vollbackup)",
        );
    }

    for (i, dir) in directories.iter().enumerate() {
        trace(&format!("loop[{}/{}] {}", i+1, total, dir));
        // Check for cancellation before each directory
        if BACKUP_CANCELLED.load(Ordering::SeqCst) {
            let _ = window.emit("backup-log", "⚠️ Backup cancelled!");
            let _ = window.emit("backup-progress", serde_json::json!({
                "progress": 0,
                "message": "Backup cancelled"
            }));
            BACKUP_CANCELLED.store(false, Ordering::SeqCst);
            return Err("Backup was cancelled".to_string());
        }

        let expanded = if dir.starts_with("~/") {
            home.join(&dir[2..])
        } else if dir == "~" {
            home.clone()
        } else {
            PathBuf::from(dir)
        };
        
        fs::symlink_metadata(&expanded).map_err(|e| format!("{}: {}", expanded.display(),e))?;
        
        let is_file = expanded.is_file();
        
        let name = expanded.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "backup".to_string());
        
        // Einzeldateien werden mit system-tar und gzip gepackt,
        // Verzeichnisse via `create_tar_gz` mit zstd (sofern verfügbar). Die
        // Endung muss zum tatsächlichen Kompressor passen, sonst schlägt die
        // zstd-Vorprüfung beim Verify/Restore unnötig fehl.
        let archive_ext = if !is_file && is_zstd_available() { "tar.zst" } else { "tar.gz" };
        let archive_name = archive_name_for(&expanded, archive_ext);
        let archive_path = backup_root.join(&archive_name);
        
        let _ = window.emit("backup-log", format!("Archiving {} ...", dir));
        let progress = 15 + (60 * (i + 1) / total);
        let _ = window.emit("backup-progress", serde_json::json!({
            "progress": progress,
            "message": format!("Archiving {}...", name)
        }));
        
        let current_snapshot = compute_snapshot(&expanded)?;
        if cached_snapshots[i].as_ref() != Some(&current_snapshot) {
            return Err(format!("Quelle seit Backup-Beginn verändert: {dir}. Schreibende Programme schließen und erneut starten."));
        }
        let source_size: u64 = current_snapshot.iter().map(|e| e.s).sum();
        trace(&format!("  compute_snapshot done ({} entries)", current_snapshot.len()));

        let mut reused_from_prev: Option<(String, String, u64)> = None; // (prev_ts, prev_hash, prev_archive_size)
        if let Some((prev_ts, prev_meta)) = &previous {
            let prev_inventory = suite_root
                .join("inventories")
                .join(prev_ts);
            if let Some(prev_snapshot) = load_manifest(&prev_inventory, &archive_name) {
                if prev_snapshot == current_snapshot {
                    // Finde passenden Eintrag in vorheriger Metadata (gleicher Pfad + Archiv-Name)
                    if let Some(prev_item) = prev_meta
                        .items
                        .iter()
                        .find(|it| it.path == *dir && it.archive == archive_name)
                    {
                        if verify_item(&suite_root.join("data").join(prev_ts), prev_item).is_ok() {
                            reused_from_prev = Some((
                                prev_ts.clone(),
                                prev_item.hash.clone(),
                                prev_item.archive_size_bytes,
                            ));
                        }
                    }
                }
            }
        }

        if let Some((prev_ts, prev_hash, prev_size)) = reused_from_prev {
            trace(&format!("  reuse path: hardlink/copy from {}", prev_ts));
            // Archiv per Hardlink wiederverwenden (Fallback: fs::copy auf anderes Volume)
            let prev_archive_path = suite_root.join("data").join(&prev_ts).join(&archive_name);
            let reused_ok = reuse_archive(&prev_archive_path, &archive_path).is_ok();

            if reused_ok {
                verify_archive_source(&archive_path, &name, &current_snapshot)?;
                ensure_unchanged(&expanded, &current_snapshot)?;
                let _ = window.emit(
                    "backup-log",
                    format!("⏭️  Keine Änderungen in {} – Archiv übernommen aus {}", dir, prev_ts),
                );
                save_manifest(&inventory_root, &archive_name, &current_snapshot)?;
                items.push(BackupItem {
                    path: dir.clone(),
                    archive: archive_name,
                    hash: prev_hash,
                    archive_size_bytes: prev_size,
                    source_size_bytes: source_size,
                });
                if let Some(it) = items.last() { append_resume_entry(&backup_root, it)?; }
                continue;
            }
            // Wiederverwendung fehlgeschlagen → regulär fortfahren
            let _ = window.emit(
                "backup-log",
                format!("⚠️  Archiv-Wiederverwendung fehlgeschlagen für {} – erstelle neu", dir),
            );
        }

        if is_file {
            trace("  archiving single file");
            create_file_archive(&expanded, &name, &archive_path)?;
            trace("  single file archive done");
        } else {
            trace(&format!("  create_tar_gz start -> {}", archive_path.display()));
            create_tar_gz(&expanded, &archive_path)?;
            trace("  create_tar_gz done");
        }
        
        // Check for cancellation after archive
        if BACKUP_CANCELLED.load(Ordering::SeqCst) {
            // Clean up partial archive
            let _ = fs::remove_file(&archive_path);
            let _ = window.emit("backup-log", "⚠️ Backup cancelled!");
            let _ = window.emit("backup-progress", serde_json::json!({
                "progress": 0,
                "message": "Backup cancelled"
            }));
            BACKUP_CANCELLED.store(false, Ordering::SeqCst);
            return Err("Backup was cancelled".to_string());
        }
        
        let archive_size = fs::metadata(&archive_path)
            .map(|m| m.len())
            .unwrap_or(0);
        trace(&format!("  hashing archive ({} bytes)", archive_size));
        let hash = hash_file(&archive_path)?;
        trace("  hash done");

        // Manifest für dieses Verzeichnis persistieren, damit spätere Backups
        // den unveränderten Zustand erkennen und das Archiv wiederverwenden können.
        save_manifest(&inventory_root, &archive_name, &current_snapshot)?;

        items.push(BackupItem {
            path: dir.clone(),
            archive: archive_name,
            hash,
            archive_size_bytes: archive_size,
            source_size_bytes: source_size,
        });
        if let Some(it) = items.last() { append_resume_entry(&backup_root, it)?; }
        trace(&format!("loop[{}/{}] done", i+1, total));
    }
    trace("main loop complete, archiving inventory");

    for (label,filename,content) in [
        ("homebrew-packages", "homebrew_packages.txt", brew_inventory),
        ("mas-apps", "mas_apps.txt", mas_inventory),
        ("vscode-extensions", "vscode_extensions.txt", vscode_inventory),
    ] {
        if let Some(content)=content {
            match label {
                "homebrew-packages" => {brew_entries(&content)?;atomic_write(&inventory_root.join("Brewfile"),content.as_bytes())?;},
                "mas-apps" => {mas_ids(&content)?;},
                _ => {extension_ids(&content)?;},
            }
            let source=inventory_root.join(filename);
            atomic_write(&source,content.as_bytes())?;
            let archive_name=format!("{label}.tar.gz");
            let archive=backup_root.join(&archive_name);
            create_file_archive(&source,filename,&archive)?;
            let item=BackupItem{path:label.into(),archive:archive_name,hash:hash_file(&archive)?,
                archive_size_bytes:fs::metadata(&archive).map_err(|e| e.to_string())?.len(),source_size_bytes:content.len() as u64};
            append_resume_entry(&backup_root,&item)?;
            items.push(item);
        }
    }

    // Optional: complete Homebrew cache; selected sources must never be silently skipped.
    trace(&format!("config: brew_cache={} safari={}", config.backup_homebrew_cache, config.backup_safari_settings));
    if config.backup_homebrew_cache {
        trace("archiving homebrew-cache");
        let _ = window.emit("backup-log", "Checking Homebrew cache...");
        
        // Homebrew cache locations
        let cache_paths = [
            PathBuf::from("/opt/homebrew/var/homebrew/cache"),
            PathBuf::from("/usr/local/var/homebrew/cache"),
            dirs::home_dir().unwrap_or_default().join("Library/Caches/Homebrew"),
        ];

        let mut cache_path: Option<PathBuf> = None;
        for path in &cache_paths {
            if path.exists() {
                cache_path = Some(path.clone());
                break;
            }
        }
        
        if let Some(cache_dir) = cache_path {
            validate_source_target(&cache_dir, Path::new(&target_path))?;
            let cache_manifest=compute_snapshot(&cache_dir)?;
            let cache_size=cache_manifest.iter().map(|e| e.s).sum::<u64>();
            extra_source_guards.push((cache_dir.clone(),cache_manifest));
            
            {
                let cache_archive_name = if is_zstd_available() { "homebrew-cache.tar.zst" } else { "homebrew-cache.tar.gz" };
                let cache_archive_path = backup_root.join(cache_archive_name);
                
                let _ = window.emit("backup-log", format!("Archiving Homebrew cache ({:.1} MB)...", cache_size as f64 / (1024.0 * 1024.0)));
                
                {
                    create_tar_gz(&cache_dir, &cache_archive_path)?;
                    let archive_size = fs::metadata(&cache_archive_path).map(|m| m.len()).unwrap_or(0);
                    {
                        let hash = hash_file(&cache_archive_path)?;
                        items.push(BackupItem {
                            path: "homebrew-cache".to_string(),
                            archive: cache_archive_name.to_string(),
                            hash,
                            archive_size_bytes: archive_size,
                            source_size_bytes: cache_size,
                        });
                        if let Some(it) = items.last() { append_resume_entry(&backup_root, it)?; }
                        let _ = window.emit("backup-log", format!("✅ Homebrew cache archived: {:.1} MB", archive_size as f64 / (1024.0 * 1024.0)));
                    }
                }
            }
        } else { return Err("Homebrew-Cache ausgewählt, aber kein Cache gefunden".into()); }
    }

    // Optional: Backup Safari Settings including Bookmarks
    if config.backup_safari_settings {
        trace("archiving safari-settings");
        let _ = window.emit("backup-log", "Backing up Safari settings...");
        
        let home = dirs::home_dir().unwrap_or_default();
        let safari_paths = vec![
            // Safari Bookmarks
            home.join("Library/Safari/Bookmarks.plist"),
            // Safari History (optional, can be large)
            // home.join("Library/Safari/History.db"),
            // Safari Reading List
            home.join("Library/Safari/ReadingListArchives"),
            // Safari Extensions
            home.join("Library/Safari/Extensions"),
            // Safari Preferences
            home.join("Library/Preferences/com.apple.Safari.plist"),
            // Safari Sandbox data (contains tabs, etc.)
            home.join("Library/Containers/com.apple.Safari/Data/Library/Preferences"),
            // Safari Favorites icons
            home.join("Library/Safari/Favicon Cache"),
            // Top Sites
            home.join("Library/Safari/TopSites.plist"),
            // Last Session
            home.join("Library/Safari/LastSession.plist"),
        ];
        
        let safari_stage = PrivateDir::temp()?;
        let temp_safari_dir = safari_stage.0.join("safari_backup");
        fs::create_dir(&temp_safari_dir).map_err(|e| e.to_string())?;
        
        let mut copied_count = 0;
        for safari_path in &safari_paths {
            match fs::symlink_metadata(safari_path) {
                Err(e) if e.kind()==std::io::ErrorKind::NotFound => { continue; }
                Err(e) => return Err(format!("Safari {}: {e}",safari_path.display())),
                Ok(_) => {}
            }
            {
                let original_manifest=compute_snapshot(safari_path)?;
                extra_source_guards.push((safari_path.clone(),original_manifest));
                let relative_name = safari_path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                
                let dest = temp_safari_dir.join(&relative_name);
                
                let mut cmd=Command::new("/usr/bin/ditto");
                cmd.arg(safari_path).arg(&dest);
                let output=run_with_timeout(cmd,std::time::Duration::from_secs(3600))?;
                require_success("Safari backup copy",&output)?;
                copied_count += 1;
            }
        }
        
        if copied_count > 0 {
            let safari_archive_name = if is_zstd_available() { "safari-settings.tar.zst" } else { "safari-settings.tar.gz" };
            let safari_archive_path = backup_root.join(safari_archive_name);
            
            {
                create_tar_gz(&temp_safari_dir, &safari_archive_path)?;
                let source_size = compute_snapshot(&temp_safari_dir)?.iter().map(|e| e.s).sum();
                let archive_size = fs::metadata(&safari_archive_path).map(|m| m.len()).unwrap_or(0);
                
                {
                    let hash = hash_file(&safari_archive_path)?;
                    items.push(BackupItem {
                        path: "safari-settings".to_string(),
                        archive: safari_archive_name.to_string(),
                        hash,
                        archive_size_bytes: archive_size,
                        source_size_bytes: source_size,
                    });
                    if let Some(it) = items.last() { append_resume_entry(&backup_root, it)?; }
                    let _ = window.emit("backup-log", format!("✅ Safari settings archived: {} files/folders", copied_count));
                }
            }
        } else {
            return Err("Safari-Sicherung ausgewählt, aber keine Safari-Daten gefunden".into());
        }
        
        let _ = fs::remove_dir_all(&temp_safari_dir);
    }

    let _ = window.emit("backup-log", "Abschlussprüfung: alle Quellen und Archive werden erneut geprüft …");
    for (i,dir) in directories.iter().enumerate() {
        let source=if dir=="~" {home.clone()} else if let Some(rel)=dir.strip_prefix("~/") {home.join(rel)} else {PathBuf::from(dir)};
        extra_source_guards.push((source,cached_snapshots[i].clone().ok_or("Quellmanifest fehlt")?));
    }

    let end = Local::now();
    let end_time_str = end.format("%d.%m.%Y %H:%M:%S").to_string();
    let duration = (end - start).num_seconds() as u64;
    
    let total_size: u64 = items.iter().map(|i| i.source_size_bytes).sum();
    
    let metadata = BackupMetadata {
        timestamp: timestamp.clone(),
        items,
        hash_algorithm: "sha256".to_string(),
        total_source_size_bytes: total_size,
        start_time: start_time_str.clone(),
        end_time: end_time_str.clone(),
        duration_seconds: duration,
    };
    
    finish_backup(&backup_root,&metadata,&extra_source_guards)?;

    // Backup erfolgreich abgeschlossen — Resume-State kann jetzt verworfen werden.
    clear_resume_state(&backup_root);
    
    // Copy the DMG installer to backup root (always include app in backup)
    let dmg_filename = "macOS Backup Suite.dmg";
    let dmg_dest = suite_root.join(dmg_filename);
    let mut dmg_copied = false;
    
    // Look for DMG in the app bundle's Resources folder
    if let Ok(exe) = std::env::current_exe() {
        // exe is at: App.app/Contents/MacOS/binary
        // We need: App.app/Contents/Resources/
        if let Some(macos_dir) = exe.parent() {
            let resources_dmg = macos_dir.parent()
                .map(|contents| contents.join("Resources").join(dmg_filename));
            
            if let Some(ref src) = resources_dmg {
                if src.exists() {
                    if fs::copy(src, &dmg_dest).is_ok() {
                        let _ = window.emit("backup-log", format!("✅ App installer copied: {}", dmg_filename));
                        dmg_copied = true;
                    }
                }
            }
        }
    }
    
    // Fallback: Look in multiple locations
    if !dmg_copied {
        let home = dirs::home_dir().unwrap_or_default();
        let mut candidates: Vec<PathBuf> = Vec::new();

        // Dev-Build: beliebige Version im dmg-Bundle-Ordner suchen
        let dmg_dirs = [
            PathBuf::from("src-tauri/target/release/bundle/dmg"),
            home.join("Documents/GitHub/macos-backup-tauri/src-tauri/target/release/bundle/dmg"),
        ];
        for d in &dmg_dirs {
            if let Ok(rd) = fs::read_dir(d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|s| s.to_str()) == Some("dmg") {
                        candidates.push(p);
                    }
                }
            }
        }

        // Resource-Pfade im .app-Bundle
        candidates.extend([
            PathBuf::from("src-tauri/target/release/bundle/macos/macOS Backup Suite.app/Contents/Resources/macOS Backup Suite.dmg"),
            home.join("Documents/GitHub/macos-backup-tauri/src-tauri/target/release/bundle/macos/macOS Backup Suite.app/Contents/Resources/macOS Backup Suite.dmg"),
            PathBuf::from("/Applications/macOS Backup Suite.app/Contents/Resources/macOS Backup Suite.dmg"),
        ]);

        for dev_path in &candidates {
            if dev_path.exists() {
                if fs::copy(dev_path, &dmg_dest).is_ok() {
                    let _ = window.emit("backup-log", format!("✅ App installer copied: {}", dmg_filename));
                    dmg_copied = true;
                    break;
                }
            }
        }
    }
    
    if !dmg_copied {
        let _ = window.emit("backup-log", "ℹ️ App installer (DMG) not found - run 'npm run tauri build'");
    }
    
    let latest = serde_json::json!({
        "latest": timestamp,
        "created_at": end.to_rfc3339()
    });
    if let Err(e)=atomic_write(&suite_root.join("latest.json"), latest.to_string().as_bytes()) {
        let _ = window.emit("backup-log", format!("Backup vollständig; Aktualisierung der Latest-Verknüpfung fehlgeschlagen: {e}"));
    }
    
    let duration_str = if duration >= 3600 {
        format!("{}h {}m {}s", duration / 3600, (duration % 3600) / 60, duration % 60)
    } else if duration >= 60 {
        format!("{}m {}s", duration / 60, duration % 60)
    } else {
        format!("{}s", duration)
    };
    
    let _ = window.emit("backup-log", format!("=== Backup finished: {} (Duration: {}) ===", end_time_str, duration_str));
    let _ = window.emit("backup-progress", serde_json::json!({
        "progress": 100,
        "message": "Backup completed."
    }));
    
    Ok(metadata)
}

#[tauri::command]
async fn verify_backup(
    window: tauri::Window,
    target_path: String,
    timestamp: String,
) -> Result<VerifyResult, String> {
    validate_component(&timestamp)?;
    tauri::async_runtime::spawn_blocking(move || {
        verify_backup_impl(window, target_path, timestamp)
    })
    .await
    .map_err(|e| format!("Verify task join error: {}", e))?
}

fn verify_backup_impl(
    window: tauri::Window,
    target_path: String,
    timestamp: String,
) -> Result<VerifyResult, String> {
    let _guard = OperationGuard::acquire()?;
    validate_component(&timestamp)?;
    let backup_path = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("data")
        .join(&timestamp);
    
    let metadata_path = backup_path.join("metadata.json");
    if !metadata_path.exists() {
        return Err(format!("Backup not found: {}", timestamp));
    }
    let metadata = load_backup_metadata(&metadata_path)?;
    
    let total_files = metadata.items.len();
    let mut verified_files = 0;
    let mut failed_files = Vec::new();

    if total_files == 0 {
        let _ = window.emit("backup-log", "Keine Dateien im Backup zum Verifizieren.");
        return Ok(VerifyResult {
            success: true,
            total_files: 0,
            verified_files: 0,
            failed_files: Vec::new(),
            message: "Keine Dateien zum Verifizieren".to_string(),
        });
    }

    for (i, item) in metadata.items.iter().enumerate() {
        // Check for cancellation
        if VERIFY_CANCELLED.load(Ordering::SeqCst) {
            VERIFY_CANCELLED.store(false, Ordering::SeqCst);
            return Err("Verification cancelled".to_string());
        }
        
        let archive_path = backup_path.join(&item.archive);
        
        let progress_msg = format!("Verifiziere {}/{}: {}", i + 1, total_files, item.archive);
        let _ = window.emit("backup-log", progress_msg);
        
        if !archive_path.exists() {
            failed_files.push(format!("{}: File not found", item.archive));
            continue;
        }
        
        match hash_file(&archive_path) {
            Ok(computed_hash) => {
                if computed_hash.eq_ignore_ascii_case(&item.hash) {
                    verified_files += 1;
                } else {
                    failed_files.push(format!("{}: Hash mismatch (expected: {}, computed: {})", 
                        item.archive, &item.hash.chars().take(16).collect::<String>(), &computed_hash[..16]));
                }
            }
            Err(e) => {
                failed_files.push(format!("{}: Read error: {}", item.archive, e));
            }
        }
        
        // Emit progress
        let fraction = (i + 1) as f64 / total_files as f64;
        let _ = window.emit("backup-progress", ProgressUpdate {
            message: format!("{}/{} files verified", i + 1, total_files),
            fraction,
        });
    }
    
    let success = failed_files.is_empty();
    let message = if success {
        format!("All {} files verified successfully!", total_files)
    } else {
        format!("{} of {} files failed", failed_files.len(), total_files)
    };
    
    let _ = window.emit("backup-log", &message);
    
    Ok(VerifyResult {
        success,
        total_files,
        verified_files,
        failed_files,
        message,
    })
}

/// Parallel backup verification with SHA-256 hash checking
/// Provides ~40% time savings for integrity checks
#[tauri::command]
async fn verify_backup_parallel(
    window: tauri::Window,
    target_path: String,
    timestamp: String,
) -> Result<VerifyResult, String> {
    validate_component(&timestamp)?;
    tauri::async_runtime::spawn_blocking(move || {
        verify_backup_parallel_impl(window, target_path, timestamp)
    })
    .await
    .map_err(|e| format!("Verify task join error: {}", e))?
}

fn verify_backup_parallel_impl(
    window: tauri::Window,
    target_path: String,
    timestamp: String,
) -> Result<VerifyResult, String> {
    let _guard = OperationGuard::acquire()?;
    validate_component(&timestamp)?;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Mutex;
    
    let backup_path = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("data")
        .join(&timestamp);
    
    let metadata_path = backup_path.join("metadata.json");
    if !metadata_path.exists() {
        return Err(format!("Backup not found: {}", timestamp));
    }
    let metadata = load_backup_metadata(&metadata_path)?;
    
    let total_files = metadata.items.len();
    let verified_counter = Arc::new(AtomicUsize::new(0));
    let failed_files = Arc::new(Mutex::new(Vec::<String>::new()));
    
    let _ = window.emit("backup-log", format!("🔍 Parallel verification of {} files...", total_files));
    
    // Process files in parallel batches (4 at a time to balance CPU and I/O)
    const PARALLEL_VERIFY: usize = 4;
    
    let items: Vec<_> = metadata.items.iter().cloned().collect();
    let chunks: Vec<Vec<BackupItem>> = items
        .chunks(PARALLEL_VERIFY)
        .map(|c| c.to_vec())
        .collect();
    
    let mut processed = 0;
    
    for chunk in chunks {
        // Abbruch zwischen den Batches sauber behandeln: laufende hash_file-
        // Aufrufe brechen intern via VERIFY_CANCELLED ab und würden sonst
        // fälschlich als „Read error“ in failed_files landen. Hier stattdessen
        // kontrolliert mit klarer Meldung aussteigen.
        if VERIFY_CANCELLED.load(Ordering::SeqCst) {
            VERIFY_CANCELLED.store(false, Ordering::SeqCst);
            return Err("Verification cancelled".to_string());
        }
        let mut handles = Vec::new();
        
        for item in chunk {
            let backup_path_clone = backup_path.clone();
            let verified = Arc::clone(&verified_counter);
            let failed = Arc::clone(&failed_files);
            
            let handle = std::thread::spawn(move || {
                let archive_path = backup_path_clone.join(&item.archive);
                
                if !archive_path.exists() {
                    let mut failed_lock = failed.lock().unwrap();
                    failed_lock.push(format!("{}: File not found", item.archive));
                    return;
                }
                
                match hash_file(&archive_path) {
                    Ok(computed_hash) => {
                        if computed_hash.eq_ignore_ascii_case(&item.hash) {
                            verified.fetch_add(1, AtomicOrdering::SeqCst);
                        } else {
                            let mut failed_lock = failed.lock().unwrap();
                            failed_lock.push(format!("{}: Hash mismatch (expected: {}, computed: {})", 
                                item.archive, &item.hash.chars().take(16).collect::<String>(), &computed_hash[..16]));
                        }
                    }
                    Err(e) => {
                        let mut failed_lock = failed.lock().unwrap();
                        failed_lock.push(format!("{}: Read error: {}", item.archive, e));
                    }
                }
            });
            
            handles.push(handle);
        }
        
        // Wait for batch to complete
        for handle in handles {
            handle.join().map_err(|_| "Verification worker failed".to_string())?;
        }
        
        processed += PARALLEL_VERIFY.min(total_files - processed);
        let fraction = processed as f64 / total_files as f64;
        let _ = window.emit("backup-progress", ProgressUpdate {
            message: format!("{}/{} files verified", processed, total_files),
            fraction,
        });
    }
    
    // Falls der Abbruch erst während des letzten Batches eintraf.
    if VERIFY_CANCELLED.load(Ordering::SeqCst) {
        VERIFY_CANCELLED.store(false, Ordering::SeqCst);
        return Err("Verification cancelled".to_string());
    }
    
    let verified_files = verified_counter.load(AtomicOrdering::SeqCst);
    let failed_files_result = match Arc::try_unwrap(failed_files) {
        Ok(mutex) => mutex.into_inner().map_err(|_| "Verification results unavailable")?,
        Err(arc) => arc.lock().map_err(|_| "Verification results unavailable")?.clone(),
    };
    
    let success = failed_files_result.is_empty() && verified_files == total_files;
    let message = if success {
        format!("✅ All {} files verified successfully (parallel)!", total_files)
    } else {
        format!("❌ {} of {} files failed", failed_files_result.len(), total_files)
    };
    
    let _ = window.emit("backup-log", &message);
    
    Ok(VerifyResult {
        success,
        total_files,
        verified_files,
        failed_files: failed_files_result,
        message,
    })
}


#[tauri::command]
fn list_backup_files(target_path: String, timestamp: String) -> Result<BackupDetails, String> {
    validate_component(&timestamp)?;
    let backup_path = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("data")
        .join(&timestamp);
    
    let metadata_path = backup_path.join("metadata.json");
    if !metadata_path.exists() {
        return Err(format!("Backup not found: {}", timestamp));
    }
    let metadata = load_backup_metadata(&metadata_path)?;
    
    let items: Vec<BackupFileInfo> = metadata.items.iter().map(|item| {
        BackupFileInfo {
            path: item.path.clone(),
            archive: item.archive.clone(),
            archive_size_bytes: item.archive_size_bytes,
            source_size_bytes: item.source_size_bytes,
        }
    }).collect();
    
    let total_archive_size_bytes: u64 = items.iter().map(|i| i.archive_size_bytes).sum();
    
    Ok(BackupDetails {
        timestamp: metadata.timestamp,
        items,
        total_source_size_bytes: metadata.total_source_size_bytes,
        total_archive_size_bytes,
        start_time: metadata.start_time,
        end_time: metadata.end_time,
        duration_seconds: metadata.duration_seconds,
    })
}

#[tauri::command]
fn list_backups(target_path: String) -> Result<Vec<BackupListItem>, String> {
    let data_path = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("data");
    
    if !data_path.exists() {
        return Ok(Vec::new());
    }
    
    let mut backups = Vec::new();
    if let Ok(entries) = fs::read_dir(&data_path) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    let metadata_path = entry.path().join("metadata.json");
                    let metadata_valid = load_backup_metadata(&metadata_path).is_ok();
                    let hash_verified = false;
                    
                    backups.push(BackupListItem {
                        timestamp: name.to_string(),
                        hash_verified,
                        metadata_valid,
                    });
                }
            }
        }
    }
    
    backups.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(backups)
}

#[tauri::command]
fn get_manual_apps_from_backup(target_path: String, timestamp: String) -> Result<Vec<String>, String> {
    validate_component(&timestamp)?;
    let inventory_path = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("inventories")
        .join(&timestamp)
        .join("manual_apps.txt");
    
    if !inventory_path.exists() {
        return Err("File manual_apps.txt not found".to_string());
    }
    
    let content = fs::read_to_string(&inventory_path)
        .map_err(|e| format!("Error reading file: {}", e))?;
    
    let apps: Vec<String> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    
    Ok(apps)
}

#[tauri::command]
fn save_license_data(target_path: String, timestamp: String, data: Vec<AppLicenseEntry>) -> Result<(), String> {
    validate_component(&timestamp)?;
    let inventory_dir = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("inventories")
        .join(&timestamp);
    
    if !inventory_dir.exists() {
        return Err("Inventory directory not found".to_string());
    }
    
    let license_path = inventory_dir.join("license_data.json");
    let json = serde_json::to_string_pretty(&data)
        .map_err(|e| format!("JSON serialization error: {}", e))?;
    
    fs::write(&license_path, &json)
        .map_err(|e| format!("Error writing license data: {}", e))?;
    
    Ok(())
}

#[tauri::command]
fn load_license_data(target_path: String, timestamp: String) -> Result<Vec<AppLicenseEntry>, String> {
    validate_component(&timestamp)?;
    let license_path = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("inventories")
        .join(&timestamp)
        .join("license_data.json");
    
    if !license_path.exists() {
        return Ok(Vec::new());
    }
    
    let content = fs::read_to_string(&license_path)
        .map_err(|e| format!("Error reading license data: {}", e))?;
    
    let data: Vec<AppLicenseEntry> = serde_json::from_str(&content)
        .map_err(|e| format!("JSON parse error: {}", e))?;
    
    Ok(data)
}

#[tauri::command]
fn show_help_window(app_handle: tauri::AppHandle) -> Result<(), String> {
    use tauri::WebviewUrl;
    
    // Check if help window already exists
    if let Some(window) = app_handle.get_webview_window("help") {
        window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    
    // Create help window
    let help_window = tauri::WebviewWindowBuilder::new(
        &app_handle,
        "help",
        WebviewUrl::App("help.html".into())
    )
    .title("macOS Backup Suite - Hilfe")
    .inner_size(800.0, 600.0)
    .resizable(true)
    .build()
    .map_err(|e| e.to_string())?;
    
    help_window.set_focus().map_err(|e| e.to_string())?;
    
    Ok(())
}

#[tauri::command]
async fn restore_items(
    target_path: String,
    timestamp: String,
    items: Vec<String>,
    overwrite: bool,
    window: tauri::Window,
) -> Result<RestoreResult, String> {
    validate_component(&timestamp)?;
    tauri::async_runtime::spawn_blocking(move || {
        restore_items_impl(target_path, timestamp, items, overwrite, window)
    })
    .await
    .map_err(|e| format!("Restore task join error: {}", e))?
}

/// Ergebnis eines Test-Restores.
#[derive(Serialize, Debug, Clone)]
pub struct TestRestoreResult {
    pub item_path: String,
    pub archive: String,
    pub dest_dir: String,
    pub extracted_path: String,
    pub bytes_extracted: u64,
    pub file_count: u64,
}

/// Test-Restore: extrahiert genau ein Backup-Item in einen frei gewählten
/// Zielordner. Schreibt **nur** in einen Unterordner unterhalb von `dest_dir`,
/// niemals an den ursprünglichen Pfad. Dadurch lässt sich ein Backup
/// zerstörungsfrei verifizieren.
///
/// Sicherheitsregeln:
/// * `dest_dir` muss existieren und ein Verzeichnis sein.
/// * Ein privater, atomar angelegter Unterordner wird nach erfolgreicher
///   Hash- und Inhaltsprüfung zur Verfügung gestellt.
/// * Spezial-Items wie `homebrew-packages`, `mas-apps`, `vscode-extensions`
///   werden abgelehnt — Test-Restore unterstützt nur Datei-/Ordner-Archive.
#[tauri::command]
async fn test_restore_item(
    target_path: String,
    timestamp: String,
    item_path: String,
    dest_dir: String,
    window: tauri::Window,
) -> Result<TestRestoreResult, String> {
    validate_component(&timestamp)?;
    tauri::async_runtime::spawn_blocking(move || {
        test_restore_item_impl(target_path, timestamp, item_path, dest_dir, window)
    })
    .await
    .map_err(|e| format!("Test-Restore task join error: {}", e))?
}

fn test_restore_item_impl(
    target_path: String,
    timestamp: String,
    item_path: String,
    dest_dir: String,
    window: tauri::Window,
) -> Result<TestRestoreResult, String> {
    let _guard = OperationGuard::acquire()?;
    validate_component(&timestamp)?;
    let backup = PathBuf::from(&target_path)
        .join("macos-backup-suite/data")
        .join(&timestamp);
    let result = test_restore_to(&backup, &item_path, Path::new(&dest_dir))?;
    let _ = window.emit(
        "restore-log",
        format!(
            "Test-Restore verified and extracted: {}",
            result.extracted_path
        ),
    );
    let _ = window.emit(
        "restore-progress",
        serde_json::json!({"progress":100,"message":"Test-Restore completed"}),
    );
    Ok(result)
}

fn restore_items_impl(
    target_path: String,
    timestamp: String,
    items: Vec<String>,
    overwrite: bool,
    window: tauri::Window,
) -> Result<RestoreResult, String> {
    let _guard = OperationGuard::acquire()?;
    validate_component(&timestamp)?;
    let backup = PathBuf::from(&target_path)
        .join("macos-backup-suite/data")
        .join(&timestamp);
    let home = dirs::home_dir().ok_or("Home directory not found")?;
    restore_selected(&backup, &items, overwrite, &home, Some(&window))
}


fn restore_homebrew_packages(
    backup_path: &Path,
    archive_name: &str,
    reinstall: bool,
    window: Option<&tauri::Window>,
) -> Result<usize, String> {
    let content = read_inventory(&backup_path.join(archive_name), "homebrew_packages.txt")?;
    let entries = brew_entries(&content)?;
    if entries.is_empty() {
        return Ok(0);
    }
    let brew = find_brew_path().ok_or("Homebrew not installed")?;
    if let Some(w) = window {
        let _ = w.emit(
            "restore-log",
            "Installing Brewfile package names. Bundle service/link options are not applied.",
        );
    }
    install_brew_entries(&brew, &entries, reinstall, window)
}


/// Quick-Restore mode: Install essential packages first for rapid productivity
/// Essential brew packages: git, vim, python, node, curl, wget, htop, tree, jq, ripgrep
/// Essential casks: visual-studio-code, iterm2, google-chrome, firefox, 1password
#[tauri::command]
async fn quick_restore_essentials(
    target_path: String,
    timestamp: String,
    window: tauri::Window,
) -> Result<RestoreResult, String> {
    validate_component(&timestamp)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OperationGuard::acquire()?;
        let backup = PathBuf::from(target_path)
            .join("macos-backup-suite/data")
            .join(timestamp);
        let home = dirs::home_dir().ok_or("Home directory not found")?;
        quick_restore(&backup, &home, Some(&window))
    })
    .await
    .map_err(|e| format!("Quick restore task failed: {e}"))?
}


/// Restore Safari settings from backup
fn restore_safari_settings(
    backup_path: &Path,
    archive_name: &str,
    home: &Path,
    overwrite: bool,
) -> Result<MergeResult, String> {
    let archive = backup_path.join(archive_name);
    let stage = PrivateDir::temp()?;
    extract_archive_to(&archive, &stage.0)?;
    let root = stage.0.join("safari_backup");
    let md = fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
    if !md.is_dir() || md.file_type().is_symlink() {
        return Err("Safari archive root must be a directory".into());
    }
    let destinations = [
        ("Bookmarks.plist", "Library/Safari/Bookmarks.plist"),
        ("ReadingListArchives", "Library/Safari/ReadingListArchives"),
        ("Extensions", "Library/Safari/Extensions"),
        ("TopSites.plist", "Library/Safari/TopSites.plist"),
        ("LastSession.plist", "Library/Safari/LastSession.plist"),
        (
            "Preferences",
            "Library/Containers/com.apple.Safari/Data/Library/Preferences",
        ),
        (
            "com.apple.Safari.plist",
            "Library/Preferences/com.apple.Safari.plist",
        ),
        ("Favicon Cache", "Library/Safari/Favicon Cache"),
    ];
    let mut result = MergeResult::default();
    let mut found = false;
    for (name, dest) in destinations {
        let source = root.join(name);
        match fs::symlink_metadata(&source) {
            Ok(md) => {
                found = true;
                if md.file_type().is_symlink() {
                    return Err(format!("Safari item is a symlink: {name}"));
                }
                let r = copy_then_merge(&source, &home.join(dest), overwrite)?;
                result.restored += r.restored;
                result.skipped += r.skipped;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    if !found {
        return Err("No Safari settings found in archive".into());
    }
    Ok(result)
}


/// Restore Homebrew cache from backup
fn restore_homebrew_cache(
    backup_path: &Path,
    archive_name: &str,
    home: &Path,
    overwrite: bool,
) -> Result<MergeResult, String> {
    let archive = backup_path.join(archive_name);
    let stage = PrivateDir::temp()?;
    extract_archive_to(&archive, &stage.0)?;
    let source = [stage.0.join("Homebrew"), stage.0.join("cache")]
        .into_iter()
        .find(|p| {
            fs::symlink_metadata(p)
                .map(|m| m.is_dir() && !m.file_type().is_symlink())
                .unwrap_or(false)
        })
        .ok_or("Homebrew cache root not found")?;
    copy_then_merge(&source, &home.join("Library/Caches/Homebrew"), overwrite)
}


/// Parallel MAS app installation with up to 4 concurrent downloads
/// Provides ~60-80% time savings when installing many apps
fn restore_mas_apps(
    backup_path: &Path,
    archive_name: &str,
    _reinstall: bool,
    window: Option<&tauri::Window>,
) -> Result<usize, String> {
    let ids = mas_ids(&read_inventory(
        &backup_path.join(archive_name),
        "mas_apps.txt",
    )?)?;
    if ids.is_empty() {
        return Ok(0);
    }
    let mas = find_homebrew_command("mas").ok_or("Mac App Store command 'mas' not installed")?;
    let mut list = Command::new(&mas);
    list.arg("list");
    let output = run_with_timeout(list, std::time::Duration::from_secs(60))?;
    require_success("mas list", &output)?;
    let installed: std::collections::HashSet<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    let pending: Vec<_> = ids
        .into_iter()
        .filter(|id| !installed.contains(id))
        .collect();
    let mut count = 0;
    let mut errors = Vec::new();
    for chunk in pending.chunks(4) {
        let mut handles = Vec::new();
        for id in chunk {
            let mas = mas.clone();
            let id = id.clone();
            handles.push(std::thread::spawn(move || {
                let mut cmd = Command::new(mas);
                cmd.args(["install", &id]);
                run_with_timeout(cmd, std::time::Duration::from_secs(900))
                    .and_then(|o| require_success(&format!("MAS {id}"), &o))
            }));
        }
        for h in handles {
            match h.join() {
                Ok(Ok(())) => count += 1,
                Ok(Err(e)) => errors.push(e),
                Err(_) => errors.push("MAS worker failed".into()),
            }
        }
        if let Some(w) = window {
            let _ = w.emit(
                "restore-log",
                format!(
                    "MAS: {count}/{} installed, {} failed",
                    pending.len(),
                    errors.len()
                ),
            );
        }
    }
    if errors.is_empty() {
        Ok(count)
    } else {
        Err(format!(
            "{count}/{} MAS apps installed; {}",
            pending.len(),
            errors.join("; ")
        ))
    }
}



/// Parallel VS Code extension installation with up to 6 concurrent installs
/// Provides ~60-80% time savings when installing many extensions
fn restore_vscode_extensions(
    backup_path: &Path,
    archive_name: &str,
    reinstall: bool,
) -> Result<usize, String> {
    let extensions = extension_ids(&read_inventory(
        &backup_path.join(archive_name),
        "vscode_extensions.txt",
    )?)?;
    if extensions.is_empty() {
        return Ok(0);
    }
    let code = [
        "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        "/usr/local/bin/code",
        "/opt/homebrew/bin/code",
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file())
    .map(str::to_string)
    .or_else(|| find_homebrew_command("code"))
    .ok_or("VS Code command not found")?;
    install_extensions(&code, &extensions, reinstall)
}


#[tauri::command]
fn delete_backup(target_path: String, timestamp: String) -> Result<(), String> {
    validate_component(&timestamp)?;
    let _guard = OperationGuard::acquire()?;
    let suite_root = PathBuf::from(&target_path).join("macos-backup-suite");
    
    let backup_path = suite_root.join("data").join(&timestamp);
    
    if !backup_path.exists() {
        return Err(format!("Backup {} not found", timestamp));
    }
    
    // Remove the backup data directory recursively
    fs::remove_dir_all(&backup_path)
        .map_err(|e| format!("Error deleting (data): {}", e))?;
    
    // Also remove the inventories directory for this timestamp
    let inventories_path = suite_root.join("inventories").join(&timestamp);
    if inventories_path.exists() {
        let _ = fs::remove_dir_all(&inventories_path);
    }
    
    // Update latest.json if we deleted the latest backup
    let latest_path = suite_root.join("latest.json");
    
    if latest_path.exists() {
        if let Ok(content) = fs::read_to_string(&latest_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(latest) = json.get("latest").and_then(|v| v.as_str()) {
                    if latest == timestamp {
                        // Find the next latest backup
                        let data_path = suite_root.join("data");
                        let mut backups: Vec<String> = Vec::new();
                        if let Ok(entries) = fs::read_dir(&data_path) {
                            for entry in entries.flatten() {
                                if entry.path().is_dir() {
                                    if let Some(name) = entry.file_name().to_str() {
                                        backups.push(name.to_string());
                                    }
                                }
                            }
                        }
                        backups.sort_by(|a, b| b.cmp(a));
                        
                        if let Some(new_latest) = backups.first() {
                            let new_json = serde_json::json!({
                                "latest": new_latest,
                                "created_at": chrono::Utc::now().to_rfc3339()
                            });
                            let _ = fs::write(&latest_path, serde_json::to_string_pretty(&new_json).unwrap());
                        } else {
                            // No more backups, remove latest.json
                            let _ = fs::remove_file(&latest_path);
                        }
                    }
                }
            }
        }
    }
    
    Ok(())
}

/// Retention-Policy: behält die `keep_last` neuesten Backups und/oder löscht
/// Backups, die älter als `max_age_days` sind. Mindestens eines bleibt immer
/// erhalten. Gibt die gelöschten Timestamps zurück.
#[tauri::command]
fn apply_retention_policy(
    target_path: String,
    keep_last: Option<usize>,
    max_age_days: Option<u32>,
) -> Result<Vec<String>, String> {
    let suite_root = PathBuf::from(&target_path).join("macos-backup-suite");
    let data_path = suite_root.join("data");
    if !data_path.exists() {
        return Ok(Vec::new());
    }

    // Timestamps im Format YYYYMMDD-HHMMSS (lexikografisch sortierbar ~ chronologisch)
    let mut timestamps: Vec<String> = Vec::new();
    for entry in fs::read_dir(&data_path).map_err(|e| e.to_string())?.flatten() {
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                timestamps.push(name.to_string());
            }
        }
    }
    timestamps.sort_by(|a, b| b.cmp(a)); // neueste zuerst

    if timestamps.len() <= 1 {
        return Ok(Vec::new()); // niemals alles löschen
    }

    let mut to_delete: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // keep_last: alles jenseits behalten als Löschkandidat
    if let Some(keep) = keep_last {
        let keep = keep.max(1);
        if timestamps.len() > keep {
            for ts in &timestamps[keep..] {
                to_delete.insert(ts.clone());
            }
        }
    }

    // max_age_days: alles älter löschen
    if let Some(days) = max_age_days {
        let now = chrono::Local::now();
        let cutoff = now - chrono::Duration::days(days as i64);
        for ts in &timestamps {
            if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%d-%H%M%S") {
                if let Some(dt) = parsed.and_local_timezone(chrono::Local).single() {
                    if dt < cutoff {
                        to_delete.insert(ts.clone());
                    }
                }
            }
        }
    }

    // Garantie: mindestens das neueste Backup behalten
    if let Some(newest) = timestamps.first() {
        to_delete.remove(newest);
    }

    let mut deleted = Vec::new();
    for ts in to_delete {
        match delete_backup(target_path.clone(), ts.clone()) {
            Ok(_) => deleted.push(ts),
            Err(e) => eprintln!("Retention: konnte {} nicht löschen: {}", ts, e),
        }
    }

    Ok(deleted)
}

/// Dry-Run / Preview: berechnet ohne Schreibzugriff, was ein Backup tun würde.
/// Liefert pro Verzeichnis Status (exists, size, bytes), sowie Gesamt-Summen
/// und verfügbaren Zielplatz. Nützlich als Vorschau vor dem echten Backup.
#[tauri::command]
fn dry_run_backup(
    target_path: String,
    directories: Vec<String>,
) -> Result<serde_json::Value, String> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut missing: Vec<String> = Vec::new();

    for dir in &directories {
        let expanded = if dir.starts_with("~/") {
            home.join(&dir[2..])
        } else if dir == "~" {
            home.clone()
        } else {
            PathBuf::from(dir)
        };

        if !expanded.exists() {
            missing.push(dir.clone());
            items.push(serde_json::json!({
                "path": dir,
                "exists": false,
                "bytes": 0,
                "is_file": false,
            }));
            continue;
        }

        validate_source_target(&expanded,Path::new(&target_path))?;
        let bytes = compute_snapshot(&expanded)?.iter().map(|e| e.s).sum::<u64>();
        let is_file=fs::symlink_metadata(&expanded).map_err(|e| e.to_string())?.is_file();
        total_bytes += bytes;

        let archive_ext = if !is_file && is_zstd_available() { "tar.zst" } else { "tar.gz" };
        let archive_name = archive_name_for(&expanded, archive_ext);

        items.push(serde_json::json!({
            "path": dir,
            "exists": true,
            "bytes": bytes,
            "is_file": is_file,
            "archive_name": archive_name,
        }));
    }

    // Verfügbarer Platz am Ziel (in Bytes)
    let free_gb = get_free_space_gb(Path::new(&target_path));
    let available_bytes: u64 = (free_gb * 1024.0 * 1024.0 * 1024.0) as u64;
    // Grobe Schätzung: zstd ~2.5x Kompressionsrate → halber Platz reicht meist
    let estimated_archive_bytes = total_bytes.saturating_add(total_bytes / 10);
    let required_bytes = estimated_archive_bytes.saturating_add(total_bytes);

    Ok(serde_json::json!({
        "target_path": target_path,
        "items": items,
        "missing": missing,
        "total_source_bytes": total_bytes,
        "estimated_archive_bytes": estimated_archive_bytes,
        "available_bytes": available_bytes,
        "required_bytes_including_readback": required_bytes,
        "sufficient_space": missing.is_empty() && available_bytes >= required_bytes,
        "zstd_available": is_zstd_available(),
    }))
}

// ========== Menu Building ==========

fn build_menu(app_handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let about_metadata = AboutMetadata {
        name: Some("macOS Backup Suite".to_string()),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        copyright: Some("© 2025 Norbert Jander".to_string()),
        comments: Some("Backup & Restore for macOS".to_string()),
        ..Default::default()
    };
    
    let about = PredefinedMenuItem::about(app_handle, Some("About macOS Backup Suite"), Some(about_metadata))?;
    let separator = PredefinedMenuItem::separator(app_handle)?;
    let hide = PredefinedMenuItem::hide(app_handle, Some("Hide macOS Backup Suite"))?;
    let hide_others = PredefinedMenuItem::hide_others(app_handle, Some("Hide Others"))?;
    let show_all = PredefinedMenuItem::show_all(app_handle, Some("Show All"))?;
    let quit = PredefinedMenuItem::quit(app_handle, Some("Quit macOS Backup Suite"))?;
    
    let app_menu = Submenu::with_items(
        app_handle,
        "macOS Backup Suite",
        true,
        &[&about, &separator, &hide, &hide_others, &show_all, &PredefinedMenuItem::separator(app_handle)?, &quit],
    )?;
    
    let backup_start = MenuItem::with_id(app_handle, "backup_start", "Start Backup", true, Some("CmdOrCtrl+B"))?;
    let backup_add_folder = MenuItem::with_id(app_handle, "backup_add_folder", "Add Folder...", true, Some("CmdOrCtrl+O"))?;
    let backup_refresh_volumes = MenuItem::with_id(app_handle, "backup_refresh_volumes", "Refresh Volumes", true, Some("CmdOrCtrl+R"))?;
    
    let backup_menu = Submenu::with_items(
        app_handle,
        "Backup",
        true,
        &[&backup_start, &PredefinedMenuItem::separator(app_handle)?, &backup_add_folder, &backup_refresh_volumes],
    )?;
    
    let restore_start = MenuItem::with_id(app_handle, "restore_start", "Restore...", true, Some("CmdOrCtrl+Shift+R"))?;
    let restore_verify = MenuItem::with_id(app_handle, "restore_verify", "Verify Backup", true, Some("CmdOrCtrl+V"))?;
    let restore_show_files = MenuItem::with_id(app_handle, "restore_show_files", "Show Files", true, Some("CmdOrCtrl+F"))?;
    
    let restore_menu = Submenu::with_items(
        app_handle,
        "Restore",
        true,
        &[&restore_start, &restore_verify, &PredefinedMenuItem::separator(app_handle)?, &restore_show_files],
    )?;
    
    let log_copy = MenuItem::with_id(app_handle, "log_copy", "Copy Log", true, Some("CmdOrCtrl+Shift+C"))?;
    let log_save = MenuItem::with_id(app_handle, "log_save", "Save Log...", true, Some("CmdOrCtrl+Shift+S"))?;
    let log_clear = MenuItem::with_id(app_handle, "log_clear", "Clear Log", true, Some("CmdOrCtrl+L"))?;
    
    let log_menu = Submenu::with_items(
        app_handle,
        "Log",
        true,
        &[&log_copy, &log_save, &PredefinedMenuItem::separator(app_handle)?, &log_clear],
    )?;
    
    let minimize = PredefinedMenuItem::minimize(app_handle, Some("Minimize"))?;
    let fullscreen = PredefinedMenuItem::fullscreen(app_handle, Some("Full Screen"))?;
    let close = PredefinedMenuItem::close_window(app_handle, Some("Close Window"))?;
    
    let window_menu = Submenu::with_items(
        app_handle,
        "Window",
        true,
        &[&minimize, &fullscreen, &PredefinedMenuItem::separator(app_handle)?, &close],
    )?;
    
    let help_item = MenuItem::with_id(app_handle, "show_help", "macOS Backup Suite Help", true, Some("F1"))?;
    
    let help_menu = Submenu::with_items(
        app_handle,
        "Help",
        true,
        &[&help_item],
    )?;
    
    let menu = Menu::with_items(
        app_handle,
        &[&app_menu, &backup_menu, &restore_menu, &log_menu, &window_menu, &help_menu],
    )?;
    
    app_handle.set_menu(menu)?;
    
    Ok(())
}

/// Terminate the tar process group, first politely with SIGTERM and then,
/// if the process is still alive after a short grace period, forcefully with
/// SIGKILL. Using the process group (negative PID) also reaps child zstd
/// workers spawned via `--use-compress-program`.
fn terminate_tar_process(pid: u32) {
    if pid == 0 {
        return;
    }
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
    // Poll up to ~2.5 s for graceful shutdown, then escalate to SIGKILL.
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        // kill(pid, 0) returns 0 iff the process is still alive and we have
        // permission to signal it. -1 with ESRCH means it's gone.
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !alive {
            return;
        }
    }
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[tauri::command]
fn cancel_backup() -> Result<(), String> {
    BACKUP_CANCELLED.store(true, Ordering::SeqCst);

    // Kill any running tar process (group), escalating TERM -> KILL.
    let pid = TAR_PID.swap(0, Ordering::SeqCst);
    if pid > 0 {
        // Run the escalation off the Tauri command thread so we return quickly.
        std::thread::spawn(move || terminate_tar_process(pid));
    }

    Ok(())
}

/// Cancel any ongoing operation (backup or verify)
#[tauri::command]
fn cancel_operation() -> Result<(), String> {
    BACKUP_CANCELLED.store(true, Ordering::SeqCst);
    VERIFY_CANCELLED.store(true, Ordering::SeqCst);
    
    // Kill any running tar process (group), escalating TERM -> KILL.
    let pid = TAR_PID.swap(0, Ordering::SeqCst);
    if pid > 0 {
        std::thread::spawn(move || terminate_tar_process(pid));
    }

    Ok(())
}

/// Reset the cancelled flags before starting a new operation
#[tauri::command]
fn reset_operation_state() -> Result<(), String> {
    ensure_operation_idle()?;
    BACKUP_CANCELLED.store(false, Ordering::SeqCst);
    VERIFY_CANCELLED.store(false, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn get_home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine home directory".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            load_config,
            save_config,
            get_external_volumes,
            check_homebrew,
            check_mas,
            get_brew_packages,
            get_mas_apps,
            get_manual_apps,
            get_manual_apps_from_backup,
            save_license_data,
            load_license_data,
            get_vscode_extensions,
            create_backup,
            list_backups,
            delete_backup,
            apply_retention_policy,
            dry_run_backup,
            restore_items,
            test_restore_item,
            quick_restore_essentials,
            list_backup_files,
            verify_backup,
            verify_backup_parallel,
            cancel_backup,
            cancel_operation,
            reset_operation_state,
            get_home_dir,
            list_user_folders,
            check_read_permission,
            check_full_disk_access,
            open_privacy_settings,
            restart_app,
            show_help_window,
            get_window_state,
            save_window_state,
            detect_backup_volume,
            list_resumable_backups,
            discard_resumable_backup,
        ])
        .setup(|app| {
            let app_handle = app.handle();
            
            // Restore window state from saved settings
            if let Some(window) = app.get_webview_window("main") {
                if let Some(state) = get_window_state() {
                    if state.width >= 960 && state.height >= 660 {
                        let _ = window.set_size(tauri::LogicalSize::new(state.width as f64, state.height as f64));
                    }
                    let _ = window.set_position(tauri::LogicalPosition::new(state.x as f64, state.y as f64));
                }
            }
            
            build_menu(app_handle)?;
            
            app.on_menu_event(move |app, event| {
                let id = event.id().as_ref();
                if let Some(window) = app.get_webview_window("main") {
                    match id {
                        "backup_start" => { let _ = window.eval("document.getElementById('btn-backup').click()"); }
                        "backup_add_folder" => { let _ = window.eval("document.getElementById('add-directory').click()"); }
                        "backup_refresh_volumes" => { let _ = window.eval("document.getElementById('refresh-volumes').click()"); }
                        "restore_start" => { let _ = window.eval("document.getElementById('btn-restore').click()"); }
                        "restore_verify" => { let _ = window.eval("document.getElementById('btn-restore-test').click()"); }
                        "restore_show_files" => { let _ = window.eval("document.getElementById('show-files').click()"); }
                        "log_copy" => { let _ = window.eval("document.getElementById('copy-log').click()"); }
                        "log_save" => { let _ = window.eval("document.getElementById('save-log').click()"); }
                        "log_clear" => { let _ = window.eval("document.getElementById('clear-log').click()"); }
                        "show_help" => { let _ = window.eval("showHelp()"); }
                        _ => {}
                    }
                }
            });
            
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod restore_tests;
