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
use flate2::write::GzEncoder;
use flate2::Compression;
use walkdir::WalkDir;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;

static BACKUP_CANCELLED: AtomicBool = AtomicBool::new(false);
static VERIFY_CANCELLED: AtomicBool = AtomicBool::new(false);
static OPERATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
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

/// Verify that a tar archive (zstd or gzip) exists and is structurally
/// readable by listing its contents with `tar -t`. This catches truncated or
/// corrupt archives early, before we create directories or start extracting.
fn verify_archive_integrity(archive: &Path) -> Result<(), String> {
    if !archive.exists() {
        return Err(format!("Archive not found: {}", archive.display()));
    }
    let archive_str = archive.to_string_lossy().to_string();
    let is_zst = archive_str.ends_with(".zst") || archive_str.ends_with(".tar.zst");

    let run_list = |use_zstd: bool| -> Result<std::process::Output, String> {
        let mut cmd = Command::new("tar");
        if use_zstd {
            let zstd_path = get_zstd_path()
                .ok_or_else(|| "zstd required but not installed".to_string())?;
            let compress_arg = format!("--use-compress-program={} -d", zstd_path);
            cmd.args([&compress_arg, "-tf", &archive_str]);
        } else {
            cmd.args(["-tzf", &archive_str]);
        }
        cmd.output()
            .map_err(|e| format!("tar integrity check failed: {}", e))
    };

    // Try the declared format first, then the other as a fallback (mislabeled archives).
    let first = run_list(is_zst)?;
    let listing = if first.status.success() {
        first
    } else if let Ok(second) = run_list(!is_zst) {
        if second.status.success() {
            second
        } else {
            let stderr = String::from_utf8_lossy(&first.stderr);
            return Err(format!(
                "Archive appears corrupt or unreadable ({}): {}",
                archive.display(),
                stderr.trim()
            ));
        }
    } else {
        let stderr = String::from_utf8_lossy(&first.stderr);
        return Err(format!(
            "Archive appears corrupt or unreadable ({}): {}",
            archive.display(),
            stderr.trim()
        ));
    };

    // Path-Traversal-Schutz: jede Archiv-Entry prüfen. Kein absoluter Pfad,
    // kein ".." Segment, kein Leading-Slash. macOS bsdtar entfernt diese zwar
    // standardmäßig, wir sind aber defensiv, falls eine andere tar-Variante
    // im PATH landet (z.B. GNU tar mit -P-Alias).
    let listing_str = String::from_utf8_lossy(&listing.stdout);
    for raw in listing_str.lines() {
        let entry = raw.trim_end_matches('/');
        if entry.is_empty() { continue; }
        if entry.starts_with('/') {
            return Err(format!("Archiv enthält absoluten Pfad: {}", entry));
        }
        if entry.split('/').any(|seg| seg == "..") {
            return Err(format!("Archiv enthält Pfad-Traversal-Segment: {}", entry));
        }
    }
    Ok(())
}

/// Extract a tar archive (zstd or gzip) to a target directory.
/// Tries zstd first if available, falls back to gzip for older backups.
fn extract_archive_to(archive: &Path, target_dir: &Path) -> Result<(), String> {
    // Pre-check: fail fast if the archive is missing or unreadable/corrupt.
    verify_archive_integrity(archive)?;

    fs::create_dir_all(target_dir).map_err(|e| format!("Failed to create dir: {}", e))?;

    if let Some(zstd_path) = get_zstd_path() {
        let compress_prog = format!("{} -d", zstd_path);
        let zstd_result = Command::new("tar")
            .current_dir(target_dir)
            .args(["--use-compress-program", &compress_prog, "-xf", &archive.to_string_lossy()])
            .output();

        match zstd_result {
            Ok(o) if o.status.success() => return Ok(()),
            _ => {} // Fall through to gzip
        }
    }

    // Fallback to gzip
    let output = Command::new("tar")
        .current_dir(target_dir)
        .args(["-xzf", &archive.to_string_lossy()])
        .output()
        .map_err(|e| format!("tar failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Extraction failed: {}", stderr));
    }

    Ok(())
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
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            target_volume: String::new(),
            target_directory: String::new(),
            directories: vec![
                home.join("Documents").to_string_lossy().to_string(),
                home.join("Desktop").to_string_lossy().to_string(),
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
    for item in &meta.items {
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
        if path.is_empty() {
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
    fs::write(&path, content).map_err(|e| e.to_string())
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
    mut cmd: Command,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    use std::io::Read;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn command: {}", e))?;

    let pid = child.id();
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_end(&mut stdout);
                }
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_end(&mut stderr);
                }
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Timed out: ask nicely, then force-kill.
                    unsafe { libc::kill(pid as i32, libc::SIGTERM); }
                    for _ in 0..10 {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        if matches!(child.try_wait(), Ok(Some(_))) {
                            break;
                        }
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "Command timed out after {:.0}s",
                        timeout.as_secs_f64()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("Error waiting for command: {}", e)),
        }
    }
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
    use std::sync::{Arc, Mutex};

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn command: {}", e))?;

    let pid = child.id();
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let collected_stdout = Arc::new(Mutex::new(Vec::<u8>::new()));
    let collected_stderr = Arc::new(Mutex::new(Vec::<u8>::new()));

    fn spawn_reader<R: std::io::Read + Send + 'static>(
        pipe: R,
        collected: Arc<Mutex<Vec<u8>>>,
        window: Option<tauri::Window>,
        event_name: String,
        log_prefix: String,
        stride: u32,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let reader = BufReader::new(pipe);
            let stride = stride.max(1);
            let mut counter: u32 = 0;
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(mut buf) = collected.lock() {
                    buf.extend_from_slice(line.as_bytes());
                    buf.push(b'\n');
                }
                counter = counter.wrapping_add(1);
                if counter % stride != 0 {
                    continue;
                }
                if let Some(ref w) = window {
                    let msg = if log_prefix.is_empty() {
                        line
                    } else {
                        format!("{}{}", log_prefix, line)
                    };
                    let _ = w.emit(&event_name, msg);
                }
            }
        })
    }

    let stdout_handle = stdout_pipe.map(|p| {
        spawn_reader(
            p,
            collected_stdout.clone(),
            window.cloned(),
            event_name.to_string(),
            log_prefix.to_string(),
            emit_every_n_lines,
        )
    });
    let stderr_handle = stderr_pipe.map(|p| {
        spawn_reader(
            p,
            collected_stderr.clone(),
            window.cloned(),
            event_name.to_string(),
            log_prefix.to_string(),
            emit_every_n_lines,
        )
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    unsafe { libc::kill(pid as i32, libc::SIGTERM); }
                    for _ in 0..10 {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        if matches!(child.try_wait(), Ok(Some(_))) {
                            break;
                        }
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "Command timed out after {:.0}s",
                        timeout.as_secs_f64()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("Error waiting for command: {}", e)),
        }
    };

    // Reader zu Ende lesen lassen, damit alle Zeilen mitgeloggt werden.
    if let Some(h) = stdout_handle { let _ = h.join(); }
    if let Some(h) = stderr_handle { let _ = h.join(); }

    let stdout = collected_stdout.lock().map(|g| g.clone()).unwrap_or_default();
    let stderr = collected_stderr.lock().map(|g| g.clone()).unwrap_or_default();

    Ok(std::process::Output { status, stdout, stderr })
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

fn compute_directory_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

// ========== Incremental Backup: Manifest Helpers ==========

/// Ein Manifest-Eintrag: relativer Pfad, Dateigröße in Bytes, mtime als Unix-Sekunden.
/// Unveränderte Snapshots über zwei Backups hinweg erlauben Archiv-Wiederverwendung.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ManifestEntry {
    p: String,
    s: u64,
    m: i64,
}

/// Erzeugt ein Snapshot-Manifest für eine Datei oder ein Verzeichnis.
/// Für Verzeichnisse werden nur reguläre Dateien erfasst (Symlinks werden als
/// eigene Einträge mit `s=0`, der mtime des Links aufgenommen).
fn compute_snapshot(root: &Path) -> Vec<ManifestEntry> {
    use std::time::UNIX_EPOCH;
    let mut entries: Vec<ManifestEntry> = Vec::new();

    if root.is_file() {
        if let Ok(md) = fs::symlink_metadata(root) {
            let mtime = md.modified().ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            entries.push(ManifestEntry {
                p: root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                s: md.len(),
                m: mtime,
            });
        }
        return entries;
    }

    for dent in WalkDir::new(root).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        let path = dent.path();
        if path == root { continue; }
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => continue,
        };
        let md = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let ft = md.file_type();
        if !(ft.is_file() || ft.is_symlink()) { continue; }
        let mtime = md.modified().ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = if ft.is_symlink() { 0 } else { md.len() };
        entries.push(ManifestEntry { p: rel, s: size, m: mtime });
    }

    // Deterministisch sortieren für stabilen Vergleich
    entries.sort_by(|a, b| a.p.cmp(&b.p));
    entries
}

fn manifest_path_for(inventory_root: &Path, archive_name: &str) -> PathBuf {
    inventory_root.join("manifests").join(format!("{}.json", archive_name))
}

fn save_manifest(inventory_root: &Path, archive_name: &str, entries: &[ManifestEntry]) {
    let path = manifest_path_for(inventory_root, archive_name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(entries) {
        let _ = fs::write(&path, json);
    }
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

fn append_resume_entry(backup_root: &Path, item: &BackupItem) {
    use std::io::Write;
    let path = resume_state_path(backup_root);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(item) {
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{}", json);
            let _ = f.flush();
        }
    }
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
        if p.join("metadata.json").exists() {
            continue; // bereits abgeschlossen
        }
        if !resume_state_path(&p).exists() {
            continue;
        }
        let items = load_resume_entries(&p);
        if items.is_empty() {
            continue;
        }
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
    // Pfad-Traversal-Schutz: Timestamp darf keine Separator enthalten
    if timestamp.contains('/') || timestamp.contains("..") || timestamp.is_empty() {
        return Err("Ungültiger Timestamp".to_string());
    }
    let suite_root = PathBuf::from(&target_path).join("macos-backup-suite");
    let data_dir = suite_root.join("data").join(&timestamp);
    if data_dir.exists() {
        if data_dir.join("metadata.json").exists() {
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
    use std::os::unix::process::CommandExt;
    
    // Use system tar command with zstd compression (faster than gzip, better ratio)
    //
    // Symlink-Handling (siehe README "Symlink-Handling"):
    //   * `tar` speichert Symlinks als Symlinks (kein `-h` / `--dereference`).
    //   * Manifest-Snapshots werden mit `follow_links(false)` erzeugt — reine
    //     Link-Änderungen werden erkannt, Änderungen am Symlink-Ziel außerhalb
    //     des Backup-Scopes aber nicht.
    //   * `--exclude` greift nur auf Pfade innerhalb des Quellbaumes.
    let source_parent = source.parent().unwrap_or(Path::new("/"));
    let source_name = source.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "backup".to_string());
    
    // Check if zstd is available, fallback to gzip
    let zstd_path = get_zstd_path();
    
    // Spawn the process so we can track and kill it
    let mut child = if let Some(zstd_bin) = zstd_path {
        // Kompressionsstufe konfigurierbar per Env-Var `BACKUP_ZSTD_LEVEL`
        // (1 = schnell, 3 = default, 19 = klein, 22 = maximal).
        let zstd_level: u32 = std::env::var("BACKUP_ZSTD_LEVEL")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n: &u32| (1..=22).contains(&n))
            .unwrap_or(3);
        // Use zstd compression (much faster, better compression)
        let mut cmd = Command::new("tar");
        cmd.current_dir(source_parent)
            .args([
                &format!("--use-compress-program={} -T0 -{}", zstd_bin, zstd_level),  // -T0 uses all CPU cores
                "-cf",
                &target.to_string_lossy(),
                "--exclude", "*.sock",
                "--exclude", "*/sockets/*",
                &source_name,
            ]);
        // Create new process group so we can kill all children
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        cmd.spawn().map_err(|e| format!("Failed to spawn tar with zstd: {}", e))?
    } else {
        // Fallback to gzip
        let mut cmd = Command::new("tar");
        cmd.current_dir(source_parent)
            .args([
                "-czf",
                &target.to_string_lossy(),
                "--exclude", "*.sock",
                "--exclude", "*/sockets/*",
                &source_name,
            ]);
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        cmd.spawn().map_err(|e| format!("Failed to spawn tar: {}", e))?
    };
    
    // Store PID for potential cancellation (immediately after spawn)
    let child_pid = child.id();
    TAR_PID.store(child_pid, Ordering::SeqCst);

    // Re-check cancellation flag: if cancel arrived between spawn() and storing
    // the PID, the cancel handler would not have been able to kill the process.
    // Close that race by signalling the newly-spawned process group now.
    if BACKUP_CANCELLED.load(Ordering::SeqCst) {
        terminate_tar_process(child_pid);
    }

    // Wait for completion
    let status = child.wait().map_err(|e| format!("Failed to wait for tar: {}", e))?;

    // Clear PID
    TAR_PID.store(0, Ordering::SeqCst);

    // Check if cancelled
    if BACKUP_CANCELLED.load(Ordering::SeqCst) {
        let _ = fs::remove_file(target);
        return Err("Cancelled".to_string());
    }
    
    // tar returns exit code 1 for warnings (sockets, permission denied on some files, etc.)
    // This is acceptable as long as the archive was created
    if !status.success() {
        // Exit code 1 with socket/pipe warnings is fine - archive is still valid
        if status.code() == Some(1) {
            // Check if archive was created successfully
            if target.exists() {
                return Ok(());
            }
        }
        
        // If archive exists, consider it a success despite warnings
        if target.exists() {
            return Ok(());
        }
        
        return Err("tar failed".to_string());
    }
    
    Ok(())
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

    let suite_root = PathBuf::from(&target_path).join("macos-backup-suite");

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
        if candidate.join("metadata.json").exists() {
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
    let home_pre = dirs::home_dir().unwrap_or_default();
    let mut estimated_source_bytes: u64 = 0;
    let pre_total = directories.len().max(1);
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
        if !expanded.exists() {
            trace(&format!("  -> does not exist, skip"));
            continue;
        }
        let t0 = std::time::Instant::now();
        let added = if expanded.is_file() {
            fs::metadata(&expanded).map(|m| m.len()).unwrap_or(0)
        } else {
            compute_directory_size(&expanded)
        };
        let dt = t0.elapsed().as_secs_f32();
        trace(&format!("  -> {} bytes in {:.2}s", added, dt));
        estimated_source_bytes = estimated_source_bytes.saturating_add(added);
    }
    trace(&format!("pre-flight scan done, total {} bytes", estimated_source_bytes));
    if estimated_source_bytes > 0 {
        let free_gb = get_free_space_gb(Path::new(&target_path));
        let estimated_gb = (estimated_source_bytes as f64) / (1024.0 * 1024.0 * 1024.0);
        let required_gb = estimated_gb * 1.10; // 10% margin
        let _ = window.emit(
            "backup-log",
            format!(
                "Free space check: {:.2} GB free, ~{:.2} GB source (need ≥ {:.2} GB with margin)",
                free_gb, estimated_gb, required_gb
            ),
        );
        if free_gb > 0.0 && free_gb < required_gb {
            let msg = format!(
                "Insufficient free space on target: {:.2} GB free, ~{:.2} GB required (source {:.2} GB + 10% margin). Aborting.",
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
    fs::create_dir_all(&backup_root).map_err(|e| e.to_string())?;
    fs::create_dir_all(&inventory_root).map_err(|e| e.to_string())?;
    trace("dirs created");
    
    let _ = window.emit("backup-log", format!("=== Backup started: {} ===", start_time_str));
    let _ = window.emit("backup-progress", serde_json::json!({
        "progress": 1,
        "message": "Initialisiere Backup..."
    }));
    
    let _ = window.emit("backup-log", "Collecting software inventory...");
    
    trace("calling get_brew_packages");
    if let Ok(brewfile) = get_brew_packages() {
        trace(&format!("get_brew_packages done ({} bytes)", brewfile.len()));
        let brewfile_path = inventory_root.join("Brewfile");
        let _ = fs::write(&brewfile_path, &brewfile);
        let _ = window.emit("backup-log", format!("Brewfile saved: {} entries", brewfile.lines().count()));
    } else {
        trace("get_brew_packages failed/skipped");
    }

    trace("calling get_manual_apps");
    if let Ok(manual_apps) = get_manual_apps() {
        trace(&format!("get_manual_apps done ({} apps)", manual_apps.len()));
        let manual_path = inventory_root.join("manual_apps.txt");
        let manual_content = manual_apps.join("\n");
        let _ = fs::write(&manual_path, &manual_content);
        let _ = window.emit("backup-log", format!("Manually installed apps: {} apps", manual_apps.len()));
    } else {
        trace("get_manual_apps failed");
    }

    trace("calling get_vscode_extensions");
    match get_vscode_extensions() {
        Ok(extensions) => {
            trace(&format!("get_vscode_extensions done ({} ext)", extensions.len()));
            let vscode_path = inventory_root.join("vscode_extensions.txt");
            let vscode_content = extensions.join("\n");
            let _ = fs::write(&vscode_path, &vscode_content);
            let _ = window.emit("backup-log", format!("VS Code Extensions: {} Extensions", extensions.len()));
        }
        Err(_) => {
            trace("get_vscode_extensions err/skipped");
            let _ = window.emit("backup-log", "VS Code not installed - extensions skipped");
        }
    }
    
    trace("inventory done, emitting progress 15");
    let _ = window.emit("backup-progress", serde_json::json!({
        "progress": 15,
        "message": "Inventory completed."
    }));
    
    let home = dirs::home_dir().unwrap_or_default();
    let mut items = Vec::new();
    let total = directories.len();
    trace(&format!("main loop begin, incremental={} total={}", incremental, total));

    // Resume: bereits erledigte Items übernehmen und zugehörige Pfade als „fertig" markieren.
    let mut completed_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    if is_resume {
        for it in &resumed_items {
            // Archiv muss weiterhin existieren — sonst als nicht-erledigt behandeln.
            if backup_root.join(&it.archive).exists() {
                completed_paths.insert(it.path.clone());
                items.push(it.clone());
            } else {
                let _ = window.emit(
                    "backup-log",
                    format!("⚠️  Resume: Archiv für '{}' fehlt – wird neu erstellt", it.path),
                );
            }
        }
    }

    // Inkrementelles Backup: vorheriges Backup ermitteln (Timestamp + Metadata),
    // um Manifeste vergleichen und Archive per Hardlink wiederverwenden zu können.
    trace("checking previous backup (incremental)");
    let previous = if incremental {
        load_previous_backup(&suite_root)
    } else {
        None
    };
    trace(&format!("previous backup check done, found={}", previous.is_some()));
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

        // Resume: bereits gesicherte Verzeichnisse überspringen
        if completed_paths.contains(dir) {
            let _ = window.emit(
                "backup-log",
                format!("♻️  Resume: Überspringe bereits gesichertes {}", dir),
            );
            let progress = 15 + (60 * (i + 1) / total);
            let _ = window.emit("backup-progress", serde_json::json!({
                "progress": progress,
                "message": format!("Resumed {}", dir)
            }));
            continue;
        }
        
        let expanded = if dir.starts_with("~/") {
            home.join(&dir[2..])
        } else if dir == "~" {
            home.clone()
        } else {
            PathBuf::from(dir)
        };
        
        if !expanded.exists() {
            let _ = window.emit("backup-log", format!("Skipping {} (not found)", dir));
            continue;
        }
        
        let is_file = expanded.is_file();
        
        let name = expanded.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "backup".to_string());
        
        let archive_ext = if is_zstd_available() { "tar.zst" } else { "tar.gz" };
        let archive_name = format!("{}.{}", name.to_lowercase().replace(' ', "-").replace('.', "_"), archive_ext);
        let archive_path = backup_root.join(&archive_name);
        
        let _ = window.emit("backup-log", format!("Archiving {} ...", dir));
        let progress = 15 + (60 * (i + 1) / total);
        let _ = window.emit("backup-progress", serde_json::json!({
            "progress": progress,
            "message": format!("Archiving {}...", name)
        }));
        
        let source_size = if is_file {
            fs::metadata(&expanded).map(|m| m.len()).unwrap_or(0)
        } else {
            trace(&format!("  compute_directory_size start: {}", dir));
            let s = compute_directory_size(&expanded);
            trace(&format!("  compute_directory_size done: {} bytes", s));
            s
        };

        // --- Inkrementell: Snapshot berechnen und mit Vorgänger vergleichen ---
        trace(&format!("  compute_snapshot start: {}", dir));
        let current_snapshot = compute_snapshot(&expanded);
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
                        let prev_archive_path = suite_root
                            .join("data")
                            .join(prev_ts)
                            .join(&archive_name);
                        if prev_archive_path.exists() {
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
            let mut reused_ok = false;
            if fs::hard_link(&prev_archive_path, &archive_path).is_ok() {
                trace("  hardlink ok");
                reused_ok = true;
            } else {
                trace("  hardlink failed, trying copy");
                if fs::copy(&prev_archive_path, &archive_path).is_ok() {
                    trace("  copy ok");
                    reused_ok = true;
                } else {
                    trace("  copy failed");
                }
            }

            if reused_ok {
                let _ = window.emit(
                    "backup-log",
                    format!("⏭️  Keine Änderungen in {} – Archiv übernommen aus {}", dir, prev_ts),
                );
                save_manifest(&inventory_root, &archive_name, &current_snapshot);
                items.push(BackupItem {
                    path: dir.clone(),
                    archive: archive_name,
                    hash: prev_hash,
                    archive_size_bytes: prev_size,
                    source_size_bytes: source_size,
                });
                if let Some(it) = items.last() { append_resume_entry(&backup_root, it); }
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
            let file = fs::File::create(&archive_path).map_err(|e| e.to_string())?;
            let encoder = GzEncoder::new(file, Compression::default());
            let mut archive = tar::Builder::new(encoder);
            archive.append_path_with_name(&expanded, &name).map_err(|e| e.to_string())?;
            // Finish tar archive and get back the GzEncoder, then finish the GzEncoder to flush all data
            let encoder = archive.into_inner().map_err(|e| e.to_string())?;
            encoder.finish().map_err(|e| e.to_string())?;
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
        save_manifest(&inventory_root, &archive_name, &current_snapshot);

        items.push(BackupItem {
            path: dir.clone(),
            archive: archive_name,
            hash,
            archive_size_bytes: archive_size,
            source_size_bytes: source_size,
        });
        if let Some(it) = items.last() { append_resume_entry(&backup_root, it); }
        trace(&format!("loop[{}/{}] done", i+1, total));
    }
    trace("main loop complete, archiving inventory");

    // Archive Homebrew packages as a restorable item
    if !completed_paths.contains("homebrew-packages") {
        trace("archiving homebrew-packages");
    if let Ok(brewfile) = get_brew_packages() {
        let brew_archive_name = if is_zstd_available() { "homebrew-packages.tar.zst" } else { "homebrew-packages.tar.gz" };
        let brew_archive_path = backup_root.join(brew_archive_name);
        let brew_temp = std::env::temp_dir().join("homebrew_packages.txt");
        let _ = fs::write(&brew_temp, &brewfile);
        
        if brew_temp.exists() {
            let source_size = fs::metadata(&brew_temp).map(|m| m.len()).unwrap_or(0);
            let file = fs::File::create(&brew_archive_path).map_err(|e| e.to_string())?;
            let encoder = GzEncoder::new(file, Compression::default());
            let mut archive = tar::Builder::new(encoder);
            archive.append_path_with_name(&brew_temp, "homebrew_packages.txt").map_err(|e| e.to_string())?;
            // Finish tar archive and get back the GzEncoder, then finish the GzEncoder to flush all data
            let encoder = archive.into_inner().map_err(|e| e.to_string())?;
            encoder.finish().map_err(|e| e.to_string())?;
            
            let archive_size = fs::metadata(&brew_archive_path).map(|m| m.len()).unwrap_or(0);
            let hash = hash_file(&brew_archive_path)?;
            
            items.push(BackupItem {
                path: "homebrew-packages".to_string(),
                archive: brew_archive_name.to_string(),
                hash,
                archive_size_bytes: archive_size,
                source_size_bytes: source_size,
            });
            if let Some(it) = items.last() { append_resume_entry(&backup_root, it); }
            let _ = window.emit("backup-log", format!("Homebrew packages archived: {} bytes", source_size));
        }
        let _ = fs::remove_file(&brew_temp);
    }
    } // end if !completed_paths.contains("homebrew-packages")
    
    // Archive MAS apps as a restorable item
    if !completed_paths.contains("mas-apps") {
        trace("archiving mas-apps");
    {
        let mas_temp = std::env::temp_dir().join("mas_apps.txt");
        // Primär: mas list direkt abfragen (zuverlässiger als Brewfile-Parsing)
        let mut mas_line_count: usize = 0;
        let mut have_content = false;
        match get_mas_apps() {
            Ok(mas_output) if !mas_output.trim().is_empty() => {
                mas_line_count = mas_output.lines().filter(|l| !l.trim().is_empty()).count();
                let _ = fs::write(&mas_temp, &mas_output);
                have_content = true;
            }
            _ => {
                // Fallback: aus Brewfile extrahieren
                if let Ok(brewfile) = get_brew_packages() {
                    let mas_lines: Vec<&str> = brewfile.lines()
                        .filter(|line| line.trim().starts_with("mas "))
                        .collect();
                    if !mas_lines.is_empty() {
                        mas_line_count = mas_lines.len();
                        let mas_content = mas_lines.join("\n");
                        let _ = fs::write(&mas_temp, &mas_content);
                        have_content = true;
                    }
                }
            }
        }

        if !have_content {
            let _ = window.emit("backup-log", "ℹ️ Keine App-Store-Apps gefunden (mas nicht installiert oder keine Apps)");
        }

        if mas_temp.exists() {
            let mas_archive_name = if is_zstd_available() { "mas-apps.tar.zst" } else { "mas-apps.tar.gz" };
            let mas_archive_path = backup_root.join(mas_archive_name);
            let source_size = fs::metadata(&mas_temp).map(|m| m.len()).unwrap_or(0);

            let file = fs::File::create(&mas_archive_path).map_err(|e| e.to_string())?;
            let encoder = GzEncoder::new(file, Compression::default());
            let mut archive = tar::Builder::new(encoder);
            archive.append_path_with_name(&mas_temp, "mas_apps.txt").map_err(|e| e.to_string())?;
            // Finish tar archive and get back the GzEncoder, then finish the GzEncoder to flush all data
            let encoder = archive.into_inner().map_err(|e| e.to_string())?;
            encoder.finish().map_err(|e| e.to_string())?;

            let archive_size = fs::metadata(&mas_archive_path).map(|m| m.len()).unwrap_or(0);
            let hash = hash_file(&mas_archive_path)?;

            items.push(BackupItem {
                path: "mas-apps".to_string(),
                archive: mas_archive_name.to_string(),
                hash,
                archive_size_bytes: archive_size,
                source_size_bytes: source_size,
            });
            if let Some(it) = items.last() { append_resume_entry(&backup_root, it); }
            let _ = window.emit("backup-log", format!("✅ MAS apps archived: {} Apps ({} bytes)", mas_line_count, source_size));
            let _ = fs::remove_file(&mas_temp);
        }
    }
    } // end if !completed_paths.contains("mas-apps")
    
    // Archive VS Code extensions as a restorable item
    if !completed_paths.contains("vscode-extensions") {
        trace("archiving vscode-extensions");
    if let Ok(extensions) = get_vscode_extensions() {
        let vscode_archive_name = if is_zstd_available() { "vscode-extensions.tar.zst" } else { "vscode-extensions.tar.gz" };
        let vscode_archive_path = backup_root.join(vscode_archive_name);
        let vscode_temp = std::env::temp_dir().join("vscode_extensions.txt");
        let vscode_content = extensions.join("
");
        let _ = fs::write(&vscode_temp, &vscode_content);
        
        if vscode_temp.exists() {
            let source_size = fs::metadata(&vscode_temp).map(|m| m.len()).unwrap_or(0);
            let file = fs::File::create(&vscode_archive_path).map_err(|e| e.to_string())?;
            let encoder = GzEncoder::new(file, Compression::default());
            let mut archive = tar::Builder::new(encoder);
            archive.append_path_with_name(&vscode_temp, "vscode_extensions.txt").map_err(|e| e.to_string())?;
            // Finish tar archive and get back the GzEncoder, then finish the GzEncoder to flush all data
            let encoder = archive.into_inner().map_err(|e| e.to_string())?;
            encoder.finish().map_err(|e| e.to_string())?;
            
            let archive_size = fs::metadata(&vscode_archive_path).map(|m| m.len()).unwrap_or(0);
            let hash = hash_file(&vscode_archive_path)?;
            
            items.push(BackupItem {
                path: "vscode-extensions".to_string(),
                archive: vscode_archive_name.to_string(),
                hash,
                archive_size_bytes: archive_size,
                source_size_bytes: source_size,
            });
            if let Some(it) = items.last() { append_resume_entry(&backup_root, it); }
            let _ = window.emit("backup-log", format!("VS Code extensions archived: {} extensions", extensions.len()));
        }
        let _ = fs::remove_file(&vscode_temp);
    }
    } // end if !completed_paths.contains("vscode-extensions")

    // Optional: Backup Homebrew Download Cache for offline installations (max 2GB)
    let config = load_config().unwrap_or_default();
    trace(&format!("config: brew_cache={} safari={}", config.backup_homebrew_cache, config.backup_safari_settings));
    if config.backup_homebrew_cache && !completed_paths.contains("homebrew-cache") {
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
            // Calculate cache size and limit to 2GB
            let cache_size = compute_directory_size(&cache_dir);
            const MAX_CACHE_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2GB
            
            if cache_size > 0 && cache_size <= MAX_CACHE_SIZE {
                let cache_archive_name = if is_zstd_available() { "homebrew-cache.tar.zst" } else { "homebrew-cache.tar.gz" };
                let cache_archive_path = backup_root.join(cache_archive_name);
                
                let _ = window.emit("backup-log", format!("Archiving Homebrew cache ({:.1} MB)...", cache_size as f64 / (1024.0 * 1024.0)));
                
                if create_tar_gz(&cache_dir, &cache_archive_path).is_ok() {
                    let archive_size = fs::metadata(&cache_archive_path).map(|m| m.len()).unwrap_or(0);
                    if let Ok(hash) = hash_file(&cache_archive_path) {
                        items.push(BackupItem {
                            path: "homebrew-cache".to_string(),
                            archive: cache_archive_name.to_string(),
                            hash,
                            archive_size_bytes: archive_size,
                            source_size_bytes: cache_size,
                        });
                        if let Some(it) = items.last() { append_resume_entry(&backup_root, it); }
                        let _ = window.emit("backup-log", format!("✅ Homebrew cache archived: {:.1} MB", archive_size as f64 / (1024.0 * 1024.0)));
                    }
                }
            } else if cache_size > MAX_CACHE_SIZE {
                let _ = window.emit("backup-log", format!("⚠️ Homebrew cache too large ({:.1} GB > 2 GB max), skipped", cache_size as f64 / (1024.0 * 1024.0 * 1024.0)));
            }
        }
    }

    // Optional: Backup Safari Settings including Bookmarks
    if config.backup_safari_settings && !completed_paths.contains("safari-settings") {
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
        
        let temp_safari_dir = std::env::temp_dir().join("safari_backup");
        let _ = fs::create_dir_all(&temp_safari_dir);
        
        let mut copied_count = 0;
        for safari_path in &safari_paths {
            if safari_path.exists() {
                let relative_name = safari_path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                
                let dest = temp_safari_dir.join(&relative_name);
                
                if safari_path.is_file() {
                    if fs::copy(safari_path, &dest).is_ok() {
                        copied_count += 1;
                    }
                } else if safari_path.is_dir() {
                    // Copy directory recursively
                    let _ = Command::new("cp")
                        .args(["-R", &safari_path.to_string_lossy(), &dest.to_string_lossy()])
                        .output();
                    copied_count += 1;
                }
            }
        }
        
        if copied_count > 0 {
            let safari_archive_name = if is_zstd_available() { "safari-settings.tar.zst" } else { "safari-settings.tar.gz" };
            let safari_archive_path = backup_root.join(safari_archive_name);
            
            if create_tar_gz(&temp_safari_dir, &safari_archive_path).is_ok() {
                let source_size = compute_directory_size(&temp_safari_dir);
                let archive_size = fs::metadata(&safari_archive_path).map(|m| m.len()).unwrap_or(0);
                
                if let Ok(hash) = hash_file(&safari_archive_path) {
                    items.push(BackupItem {
                        path: "safari-settings".to_string(),
                        archive: safari_archive_name.to_string(),
                        hash,
                        archive_size_bytes: archive_size,
                        source_size_bytes: source_size,
                    });
                    if let Some(it) = items.last() { append_resume_entry(&backup_root, it); }
                    let _ = window.emit("backup-log", format!("✅ Safari settings archived: {} files/folders", copied_count));
                }
            }
        } else {
            let _ = window.emit("backup-log", "⚠️ No Safari settings found");
        }
        
        let _ = fs::remove_dir_all(&temp_safari_dir);
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
    
    let metadata_json = serde_json::to_string_pretty(&metadata).map_err(|e| e.to_string())?;
    fs::write(backup_root.join("metadata.json"), &metadata_json).map_err(|e| e.to_string())?;

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
    fs::write(suite_root.join("latest.json"), latest.to_string()).map_err(|e| e.to_string())?;
    
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
                if computed_hash == item.hash {
                    verified_files += 1;
                } else {
                    failed_files.push(format!("{}: Hash mismatch (expected: {}, computed: {})", 
                        item.archive, &item.hash[..16], &computed_hash[..16]));
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
                        if computed_hash == item.hash {
                            verified.fetch_add(1, AtomicOrdering::SeqCst);
                        } else {
                            let mut failed_lock = failed.lock().unwrap();
                            failed_lock.push(format!("{}: Hash mismatch (expected: {}, computed: {})", 
                                item.archive, &item.hash[..16], &computed_hash[..16]));
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
            let _ = handle.join();
        }
        
        processed += PARALLEL_VERIFY.min(total_files - processed);
        let fraction = processed as f64 / total_files as f64;
        let _ = window.emit("backup-progress", ProgressUpdate {
            message: format!("{}/{} files verified", processed, total_files),
            fraction,
        });
    }
    
    let verified_files = verified_counter.load(AtomicOrdering::SeqCst);
    let failed_files_result = match Arc::try_unwrap(failed_files) {
        Ok(mutex) => mutex.into_inner().unwrap_or_default(),
        Err(arc) => arc.lock().unwrap().clone(),
    };
    
    let success = failed_files_result.is_empty();
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
                    let hash_verified = metadata_path.exists();
                    
                    backups.push(BackupListItem {
                        timestamp: name.to_string(),
                        hash_verified,
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
    tauri::async_runtime::spawn_blocking(move || {
        restore_items_impl(target_path, timestamp, items, overwrite, window)
    })
    .await
    .map_err(|e| format!("Restore task join error: {}", e))?
}

fn restore_items_impl(
    target_path: String,
    timestamp: String,
    items: Vec<String>,
    overwrite: bool,
    window: tauri::Window,
) -> Result<RestoreResult, String> {
    let backup_path = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("data")
        .join(&timestamp);
    
    let metadata_path = backup_path.join("metadata.json");
    if !metadata_path.exists() {
        return Err(format!("Backup not found: {}", timestamp));
    }
    let metadata = load_backup_metadata(&metadata_path)?;
    
    let home = dirs::home_dir().ok_or("Home directory not found")?;
    let mut restored: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    
    let total = items.len();
    
    for (i, item_path) in items.iter().enumerate() {
        // Progress: Start each item at a percentage, complete after operation
        let start_progress = (i * 100) / total;
        let end_progress = ((i + 1) * 100) / total;
        
        let _ = window.emit("restore-progress", serde_json::json!({
            "progress": start_progress,
            "message": format!("Restoring: {}", item_path)
        }));
        
        // Find the backup item
        let backup_item = metadata.items.iter().find(|it| &it.path == item_path);
        if backup_item.is_none() {
            errors.push(format!("{}: Not found in backup", item_path));
            continue;
        }
        let backup_item = backup_item.unwrap();
        
        // Special handling for different item types
        if item_path == "homebrew-packages" {
            let action = if overwrite { "Reinstalling" } else { "Installing missing" };
            let _ = window.emit("restore-log", format!("{} Homebrew packages...", action));
            match restore_homebrew_packages(&backup_path, &backup_item.archive, overwrite, Some(&window)) {
                Ok(count) => {
                    if count > 0 {
                        restored.push(format!("{} ({} newly installed)", item_path, count));
                        let _ = window.emit("restore-log", format!("✅ {} Homebrew packages newly installed/updated", count));
                    } else {
                        restored.push(format!("{} (all already present)", item_path));
                        let _ = window.emit("restore-log", format!("✅ All Homebrew packages were already installed"));
                    }
                }
                Err(e) => {
                    errors.push(format!("{}: {}", item_path, e));
                    let _ = window.emit("restore-log", format!("❌ Homebrew error: {}", e));
                }
            }
            let _ = window.emit("restore-progress", serde_json::json!({
                "progress": end_progress,
                "message": "Homebrew completed"
            }));
            continue;
        }
        
        if item_path == "mas-apps" {
            let action = if overwrite { "Reinstalling" } else { "Installing missing" };
            let _ = window.emit("restore-log", format!("{} Mac App Store Apps...", action));
            match restore_mas_apps(&backup_path, &backup_item.archive, overwrite, Some(&window)) {
                Ok(count) => {
                    restored.push(format!("{} ({} Apps)", item_path, count));
                    let _ = window.emit("restore-log", format!("✅ {} MAS apps installed", count));
                }
                Err(e) => {
                    errors.push(format!("{}: {}", item_path, e));
                    let _ = window.emit("restore-log", format!("❌ MAS error: {}", e));
                }
            }
            let _ = window.emit("restore-progress", serde_json::json!({
                "progress": end_progress,
                "message": "MAS apps completed"
            }));
            continue;
        }
        
        if item_path == "vscode-extensions" {
            let action = if overwrite { "Reinstalling" } else { "Installing missing" };
            let _ = window.emit("restore-log", format!("{} VS Code Extensions...", action));
            match restore_vscode_extensions(&backup_path, &backup_item.archive, overwrite) {
                Ok(count) => {
                    restored.push(format!("{} ({} Extensions)", item_path, count));
                    let _ = window.emit("restore-log", format!("✅ {} VS Code extensions installed", count));
                }
                Err(e) => {
                    errors.push(format!("{}: {}", item_path, e));
                    let _ = window.emit("restore-log", format!("❌ VS Code error: {}", e));
                }
            }
            let _ = window.emit("restore-progress", serde_json::json!({
                "progress": end_progress,
                "message": "VS Code completed"
            }));
            continue;
        }
        
        // Safari settings restore
        if item_path == "safari-settings" {
            let _ = window.emit("restore-log", "Restoring Safari settings...".to_string());
            match restore_safari_settings(&backup_path, &backup_item.archive) {
                Ok(count) => {
                    restored.push(format!("{} ({} files)", item_path, count));
                    let _ = window.emit("restore-log", format!("✅ {} Safari settings restored", count));
                }
                Err(e) => {
                    errors.push(format!("{}: {}", item_path, e));
                    let _ = window.emit("restore-log", format!("❌ Safari error: {}", e));
                }
            }
            let _ = window.emit("restore-progress", serde_json::json!({
                "progress": end_progress,
                "message": "Safari completed"
            }));
            continue;
        }
        
        // Homebrew cache restore
        if item_path == "homebrew-cache" {
            let _ = window.emit("restore-log", "Restoring Homebrew cache...".to_string());
            match restore_homebrew_cache(&backup_path, &backup_item.archive) {
                Ok(size_mb) => {
                    restored.push(format!("{} ({} MB)", item_path, size_mb));
                    let _ = window.emit("restore-log", format!("✅ Homebrew cache restored ({} MB)", size_mb));
                }
                Err(e) => {
                    errors.push(format!("{}: {}", item_path, e));
                    let _ = window.emit("restore-log", format!("❌ Homebrew cache error: {}", e));
                }
            }
            let _ = window.emit("restore-progress", serde_json::json!({
                "progress": end_progress,
                "message": "Homebrew cache completed"
            }));
            continue;
        }
        
        // Regular directory/file restore
        let archive_path = backup_path.join(&backup_item.archive);
        if !archive_path.exists() {
            errors.push(format!("{}: Archive not found", item_path));
            continue;
        }
        
        // Determine target path
        let target = if item_path.starts_with("~/") {
            home.join(&item_path[2..])
        } else if item_path.starts_with('/') {
            PathBuf::from(item_path)
        } else {
            home.join(item_path)
        };
        
        // Check if target exists
        if target.exists() && !overwrite {
            skipped.push(format!("{}: Already exists", item_path));
            let _ = window.emit("restore-log", format!("⏭️ Skipped: {} (exists)", item_path));
            continue;
        }
        
        // Extract archive
        let _ = window.emit("restore-log", format!("📦 Extracting: {}", item_path));
        match extract_tar_gz(&archive_path, &target, overwrite, Some(&window)) {
            Ok(_) => {
                restored.push(item_path.clone());
                let _ = window.emit("restore-log", format!("✅ Restored: {}", item_path));
            }
            Err(e) => {
                errors.push(format!("{}: {}", item_path, e));
                let _ = window.emit("restore-log", format!("❌ Error: {} - {}", item_path, e));
            }
        }
    }
    
    Ok(RestoreResult {
        restored_count: restored.len(),
        skipped_count: skipped.len(),
        error_count: errors.len(),
        restored,
        skipped,
        errors,
    })
}

fn extract_tar_gz(
    archive: &Path,
    target: &Path,
    overwrite: bool,
    window: Option<&tauri::Window>,
) -> Result<(), String> {
    // Pre-check: fail fast on corrupt/truncated archives.
    verify_archive_integrity(archive)?;

    // Remember whether the target already existed; used for cleanup on failure.
    let target_existed_before = target.exists();

    // Create parent directory if needed
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Error creating directory: {}", e))?;
    }

    // Check if target exists and we're not overwriting
    if !overwrite && target.exists() {
        return Err("Target already exists and overwrite is disabled".to_string());
    }
    
    let archive_str = archive.to_string_lossy().to_string();
    let extract_dir = target.parent().unwrap_or(Path::new("/"));
    
    // Determine decompression method based on file extension and available tools
    let is_zst = archive_str.ends_with(".zst") || archive_str.ends_with(".tar.zst");
    let zstd_path = get_zstd_path();

    // Live-Progress: bei vorhandenem Window Streaming mit `-v` und Throttling.
    // Ohne Window: stiller Modus (bisheriges Verhalten).
    const EXTRACT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600); // 1h
    const EXTRACT_STRIDE: u32 = 200; // jede 200. Datei loggen

    let tar_output = if is_zst && zstd_path.is_some() {
        // Use zstd decompression for .zst archives
        let compress_arg = format!("--use-compress-program={} -d", zstd_path.unwrap());
        let mut args: Vec<String> = Vec::new();
        if !overwrite { args.push("-k".to_string()); }
        if window.is_some() { args.push("-v".to_string()); }
        args.extend([compress_arg, "-xf".to_string(), archive_str.clone()]);
        let mut cmd = Command::new("tar");
        cmd.current_dir(extract_dir).args(&args);
        if let Some(w) = window {
            run_streamed(cmd, EXTRACT_TIMEOUT, Some(w), "restore-log", "📂 ", EXTRACT_STRIDE)
                .map_err(|e| format!("tar (zstd) error: {}", e))?
        } else {
            cmd.output().map_err(|e| format!("tar (zstd) error: {}", e))?
        }
    } else if is_zst && zstd_path.is_none() {
        return Err("Archive is zstd-compressed but zstd is not installed. Install with: brew install zstd".to_string());
    } else {
        // gzip (.tar.gz) - try first, then fallback to auto-detect
        let mut args: Vec<String> = Vec::new();
        if !overwrite { args.push("-k".to_string()); }
        if window.is_some() { args.push("-v".to_string()); }
        args.extend(["-xzf".to_string(), archive_str.clone()]);
        let mut cmd = Command::new("tar");
        cmd.current_dir(extract_dir).args(&args);
        let result = if let Some(w) = window {
            run_streamed(cmd, EXTRACT_TIMEOUT, Some(w), "restore-log", "📂 ", EXTRACT_STRIDE)
                .map_err(|e| format!("tar error: {}", e))?
        } else {
            cmd.output().map_err(|e| format!("tar error: {}", e))?
        };

        // If gzip fails and zstd is available, try zstd (could be a .tar.zst with wrong extension)
        if !result.status.success() && zstd_path.is_some() {
            let compress_arg = format!("--use-compress-program={} -d", zstd_path.unwrap());
            let mut args2: Vec<String> = Vec::new();
            if !overwrite { args2.push("-k".to_string()); }
            if window.is_some() { args2.push("-v".to_string()); }
            args2.extend([compress_arg, "-xf".to_string(), archive_str.clone()]);
            let mut cmd2 = Command::new("tar");
            cmd2.current_dir(extract_dir).args(&args2);
            if let Some(w) = window {
                run_streamed(cmd2, EXTRACT_TIMEOUT, Some(w), "restore-log", "📂 ", EXTRACT_STRIDE)
                    .map_err(|e| format!("tar (zstd fallback) error: {}", e))?
            } else {
                cmd2.output().map_err(|e| format!("tar (zstd fallback) error: {}", e))?
            }
        } else {
            result
        }
    };
    
    if !tar_output.status.success() {
        let tar_stderr = String::from_utf8_lossy(&tar_output.stderr);
        // -k causes error if files exist but that's expected when not overwriting
        if !(overwrite == false && tar_stderr.contains("exist")) {
            // Cleanup: if the target did not exist before this call, remove any
            // partial extraction so the user is not left with a half-restored tree.
            if !target_existed_before && target.exists() {
                if target.is_dir() {
                    let _ = fs::remove_dir_all(target);
                } else {
                    let _ = fs::remove_file(target);
                }
            }
            return Err(format!("Extraction failed: {}", tar_stderr));
        }
    }
    
    Ok(())
}

fn restore_homebrew_packages(
    backup_path: &Path,
    archive_name: &str,
    reinstall: bool,
    window: Option<&tauri::Window>,
) -> Result<usize, String> {
    let archive = backup_path.join(archive_name);
    
    // Extract to temp dir
    let temp_dir = std::env::temp_dir().join("macos-backup-restore");
    extract_archive_to(&archive, &temp_dir)?;
    
    // The file is a Brewfile, rename it for brew bundle
    let packages_file = temp_dir.join("homebrew_packages.txt");
    let brewfile = temp_dir.join("Brewfile");
    if !packages_file.exists() {
        return Err("Package list not found".to_string());
    }
    
    // Rename to Brewfile for brew bundle
    fs::rename(&packages_file, &brewfile).map_err(|e| e.to_string())?;
    
    // Count entries (brew and cask lines only, not mas - those are handled separately)
    let file_content = fs::read_to_string(&brewfile).map_err(|e| e.to_string())?;
    let count = file_content.lines()
        .filter(|l| l.starts_with("brew ") || l.starts_with("cask ") || l.starts_with("tap "))
        .count();
    
    if count == 0 {
        let _ = fs::remove_dir_all(&temp_dir);
        return Ok(0);
    }
    
    // Use brew bundle to install from Brewfile
    // --force will reinstall already installed packages
    let force_flag = if reinstall { " --force" } else { "" };
    let mut cmd = Command::new("/bin/zsh");
    cmd.args(["-l", "-c", &format!("cd {:?} && brew bundle --verbose{}", temp_dir, force_flag)]);
    // brew bundle can take a long time for many packages; cap at 2 hours.
    // Live-Progress: jede Ausgabezeile als restore-log emittieren.
    let output = run_streamed(
        cmd,
        std::time::Duration::from_secs(7200),
        window,
        "restore-log",
        "🍺 ",
        1,
    ).map_err(|e| format!("brew bundle error: {}", e))?;
    
    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
    
    // Parse output to count what was actually installed/upgraded
    let stdout = String::from_utf8_lossy(&output.stdout);
    let installed = stdout.lines()
        .filter(|l| l.starts_with("Installing ") || l.starts_with("Upgrading "))
        .count();
    let _already_present = stdout.lines()
        .filter(|l| l.starts_with("Using "))
        .count();
    
    // brew bundle returns non-zero if some packages fail, but we still count it as partial success
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Only error if completely failed
        if stderr.contains("error") && installed == 0 {
            return Err(format!("brew bundle failed: {}", stderr));
        }
    }
    
    // Return installed count, or if nothing new was installed, return the already_present count with a note
    if installed > 0 {
        Ok(installed)
    } else {
        // All packages were already present - return 0 to indicate nothing new
        Ok(0)
    }
}

/// Quick-Restore mode: Install essential packages first for rapid productivity
/// Essential brew packages: git, vim, python, node, curl, wget, htop, tree, jq, ripgrep
/// Essential casks: visual-studio-code, iterm2, google-chrome, firefox, 1password
#[tauri::command]
fn quick_restore_essentials(
    target_path: String,
    timestamp: String,
    window: tauri::Window,
) -> Result<RestoreResult, String> {
    let mut restored: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    let home = dirs::home_dir().ok_or_else(|| "Home directory not found".to_string())?;
    let backup_path = PathBuf::from(&target_path)
        .join("macos-backup-suite")
        .join("data")
        .join(&timestamp);

    let metadata_path = backup_path.join("metadata.json");
    if !metadata_path.exists() {
        return Err(format!("Backup not found: {}", timestamp));
    }
    let metadata = load_backup_metadata(&metadata_path)?;

    // ── Phase 1: SSH keys & shell configs (0-15%) ──
    let _ = window.emit("restore-log", "🔑 Phase 1/4: Restoring SSH keys & shell configs...");
    let _ = window.emit("restore-progress", serde_json::json!({
        "progress": 0,
        "message": "Phase 1: SSH & shell configs..."
    }));

    let phase1_paths = vec![
        "~/.ssh",
        "~/.gnupg",
        "~/.gitconfig",
        "~/.zshrc",
        "~/.zprofile",
        "~/.zsh_history",
        "~/.bashrc",
        "~/.bash_profile",
        "~/.bash_history",
        "~/.config/git",
    ];

    for config_path in &phase1_paths {
        if let Some(item) = metadata.items.iter().find(|it| &it.path == config_path) {
            let archive = backup_path.join(&item.archive);
            if archive.exists() {
                let target = if config_path.starts_with("~/") {
                    home.join(&config_path[2..])
                } else {
                    PathBuf::from(config_path)
                };

                if target.exists() {
                    skipped.push(format!("{} (already exists)", config_path));
                } else {
                    match extract_tar_gz(&archive, &target, false, Some(&window)) {
                        Ok(_) => {
                            restored.push(config_path.to_string());
                            let _ = window.emit("restore-log", format!("  ✅ Restored: {}", config_path));
                        }
                        Err(e) => {
                            errors.push(format!("{}: {}", config_path, e));
                            let _ = window.emit("restore-log", format!("  ❌ Error: {} - {}", config_path, e));
                        }
                    }
                }
            }
        }
    }

    // Fix SSH key permissions
    let ssh_dir = home.join(".ssh");
    if ssh_dir.exists() {
        let _ = Command::new("chmod").args(["700", &ssh_dir.to_string_lossy()]).output();
        if let Ok(entries) = fs::read_dir(&ssh_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                if !name.ends_with(".pub") && !name.starts_with("known_hosts") && name != "config" && name != "authorized_keys" {
                    let _ = Command::new("chmod").args(["600", &path.to_string_lossy()]).output();
                }
            }
        }
    }

    let _ = window.emit("restore-progress", serde_json::json!({
        "progress": 15,
        "message": "Phase 1 completed"
    }));

    // ── Phase 2: Homebrew & core CLI tools (15-50%) ──
    let _ = window.emit("restore-log", "🍺 Phase 2/4: Installing Homebrew CLI tools...");
    let _ = window.emit("restore-progress", serde_json::json!({
        "progress": 15,
        "message": "Phase 2: Homebrew CLI tools..."
    }));

    let essential_brews = vec![
        "git", "vim", "python", "node", "curl", "wget", "htop", "tree",
        "jq", "ripgrep", "fd", "bat", "fzf", "zsh-autosuggestions",
        "zsh-syntax-highlighting", "tmux",
    ];

    if let Some(brew_path) = find_brew_path() {
        // Parse backup Brewfile to find what was installed
        let mut packages_in_backup: Vec<String> = Vec::new();
        if let Some(item) = metadata.items.iter().find(|it| it.path == "homebrew-packages") {
            let archive = backup_path.join(&item.archive);
            let temp_dir = std::env::temp_dir().join("macos-backup-quick-restore");
            let _ = extract_archive_to(&archive, &temp_dir);
            let packages_file = temp_dir.join("homebrew_packages.txt");
            if packages_file.exists() {
                if let Ok(content) = fs::read_to_string(&packages_file) {
                    for line in content.lines() {
                        if line.starts_with("brew \"") {
                            if let Some(pkg) = line.split('"').nth(1) {
                                packages_in_backup.push(pkg.to_string());
                            }
                        }
                    }
                }
            }
            let _ = fs::remove_dir_all(&temp_dir);
        }

        let brews_to_install: Vec<&str> = essential_brews.iter()
            .filter(|pkg| packages_in_backup.iter().any(|b| b.contains(*pkg)))
            .cloned()
            .collect();

        let brew_count = brews_to_install.len();
        for (i, pkg) in brews_to_install.iter().enumerate() {
            let progress = 15 + ((i + 1) * 35 / brew_count.max(1));
            let _ = window.emit("restore-progress", serde_json::json!({
                "progress": progress,
                "message": format!("Installing {}...", pkg)
            }));

            let output = Command::new(&brew_path)
                .args(["install", pkg])
                .output();

            match output {
                Ok(o) if o.status.success() => {
                    restored.push(format!("brew: {}", pkg));
                    let _ = window.emit("restore-log", format!("  ✅ {} installed", pkg));
                }
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    if stderr.contains("already installed") {
                        skipped.push(format!("brew: {} (already installed)", pkg));
                    } else {
                        errors.push(format!("brew: {} - {}", pkg, stderr.lines().next().unwrap_or("")));
                    }
                }
                Err(e) => {
                    errors.push(format!("brew: {} - {}", pkg, e));
                }
            }
        }
    } else {
        let _ = window.emit("restore-log", "  ⚠️ Homebrew not found — skipping CLI tools");
    }

    let _ = window.emit("restore-progress", serde_json::json!({
        "progress": 50,
        "message": "Phase 2 completed"
    }));

    // ── Phase 3: Cask apps & VS Code extensions (50-85%) ──
    let _ = window.emit("restore-log", "📦 Phase 3/4: Installing apps & VS Code extensions...");
    let _ = window.emit("restore-progress", serde_json::json!({
        "progress": 50,
        "message": "Phase 3: Apps & VS Code..."
    }));

    let essential_casks = vec![
        "visual-studio-code", "iterm2", "google-chrome", "firefox",
        "1password", "rectangle", "alfred",
    ];

    if let Some(brew_path) = find_brew_path() {
        // Parse backup Brewfile to find casks
        let mut casks_in_backup: Vec<String> = Vec::new();
        if let Some(item) = metadata.items.iter().find(|it| it.path == "homebrew-packages") {
            let archive = backup_path.join(&item.archive);
            let temp_dir = std::env::temp_dir().join("macos-backup-quick-restore-casks");
            let _ = extract_archive_to(&archive, &temp_dir);
            let packages_file = temp_dir.join("homebrew_packages.txt");
            if packages_file.exists() {
                if let Ok(content) = fs::read_to_string(&packages_file) {
                    for line in content.lines() {
                        if line.starts_with("cask \"") {
                            if let Some(cask) = line.split('"').nth(1) {
                                casks_in_backup.push(cask.to_string());
                            }
                        }
                    }
                }
            }
            let _ = fs::remove_dir_all(&temp_dir);
        }

        let casks_to_install: Vec<&str> = essential_casks.iter()
            .filter(|cask| casks_in_backup.iter().any(|c| c.contains(*cask)))
            .cloned()
            .collect();

        let cask_count = casks_to_install.len();
        for (i, cask) in casks_to_install.iter().enumerate() {
            let progress = 50 + ((i + 1) * 25 / cask_count.max(1));
            let _ = window.emit("restore-progress", serde_json::json!({
                "progress": progress,
                "message": format!("Installing {}...", cask)
            }));

            let output = Command::new(&brew_path)
                .args(["install", "--cask", cask])
                .output();

            match output {
                Ok(o) if o.status.success() => {
                    restored.push(format!("cask: {}", cask));
                    let _ = window.emit("restore-log", format!("  ✅ {} installed", cask));
                }
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    if stderr.contains("already installed") {
                        skipped.push(format!("cask: {} (already installed)", cask));
                    } else {
                        errors.push(format!("cask: {} - {}", cask, stderr.lines().next().unwrap_or("")));
                    }
                }
                Err(e) => {
                    errors.push(format!("cask: {} - {}", cask, e));
                }
            }
        }
    }

    // Restore VS Code extensions if available
    if let Some(vscode_item) = metadata.items.iter().find(|it| it.path == "vscode-extensions") {
        let _ = window.emit("restore-log", "  🔌 Restoring VS Code extensions...");
        match restore_vscode_extensions(&backup_path, &vscode_item.archive, false) {
            Ok(count) => {
                restored.push(format!("vscode-extensions ({} extensions)", count));
                let _ = window.emit("restore-log", format!("  ✅ {} VS Code extensions installed", count));
            }
            Err(e) => {
                errors.push(format!("vscode-extensions: {}", e));
                let _ = window.emit("restore-log", format!("  ❌ VS Code error: {}", e));
            }
        }
    }

    let _ = window.emit("restore-progress", serde_json::json!({
        "progress": 85,
        "message": "Phase 3 completed"
    }));

    // ── Phase 4: User data directories (85-100%) ──
    let _ = window.emit("restore-log", "📁 Phase 4/4: Restoring essential data directories...");
    let _ = window.emit("restore-progress", serde_json::json!({
        "progress": 85,
        "message": "Phase 4: Data directories..."
    }));

    let phase4_paths = vec![
        "~/Documents",
        "~/Desktop",
        "~/Pictures",
        "~/.config",
        "~/Library/LaunchAgents",
    ];

    for data_path in &phase4_paths {
        if let Some(item) = metadata.items.iter().find(|it| &it.path == data_path) {
            let archive = backup_path.join(&item.archive);
            if archive.exists() {
                let target = if data_path.starts_with("~/") {
                    home.join(&data_path[2..])
                } else {
                    PathBuf::from(data_path)
                };

                if target.exists() {
                    skipped.push(format!("{} (already exists)", data_path));
                    let _ = window.emit("restore-log", format!("  ⏭️ Skipped: {} (exists)", data_path));
                } else {
                    match extract_tar_gz(&archive, &target, false, Some(&window)) {
                        Ok(_) => {
                            restored.push(data_path.to_string());
                            let _ = window.emit("restore-log", format!("  ✅ Restored: {}", data_path));
                        }
                        Err(e) => {
                            errors.push(format!("{}: {}", data_path, e));
                            let _ = window.emit("restore-log", format!("  ❌ Error: {} - {}", data_path, e));
                        }
                    }
                }
            }
        }
    }

    let _ = window.emit("restore-progress", serde_json::json!({
        "progress": 100,
        "message": "Quick-Restore completed"
    }));

    let _ = window.emit("restore-log", format!(
        "🎉 Quick-Restore completed: {} installed, {} skipped, {} errors",
        restored.len(), skipped.len(), errors.len()
    ));

    Ok(RestoreResult {
        restored_count: restored.len(),
        skipped_count: skipped.len(),
        error_count: errors.len(),
        restored,
        skipped,
        errors,
    })
}

/// Restore Safari settings from backup
fn restore_safari_settings(backup_path: &Path, archive_name: &str) -> Result<usize, String> {
    let archive = backup_path.join(archive_name);
    let home = dirs::home_dir().ok_or("Home directory not found")?;
    
    let temp_dir = std::env::temp_dir().join("macos-backup-restore-safari");
    let _ = fs::remove_dir_all(&temp_dir);
    extract_archive_to(&archive, &temp_dir)?;
    
    let mut restored_count = 0;
    
    // Safari paths to restore
    let safari_destinations = [
        ("Bookmarks.plist", home.join("Library/Safari/Bookmarks.plist")),
        ("ReadingListArchives", home.join("Library/Safari/ReadingListArchives")),
        ("Extensions", home.join("Library/Safari/Extensions")),
        ("TopSites.plist", home.join("Library/Safari/TopSites.plist")),
        ("LastSession.plist", home.join("Library/Safari/LastSession.plist")),
        ("Preferences", home.join("Library/Containers/com.apple.Safari/Data/Library/Preferences")),
    ];
    
    for (name, dest_path) in &safari_destinations {
        let source = temp_dir.join(name);
        if source.exists() {
            // Create parent directory
            if let Some(parent) = dest_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            
            // Copy using ditto to preserve attributes
            let output = Command::new("ditto")
                .args([&source.to_string_lossy().to_string(), &dest_path.to_string_lossy().to_string()])
                .output();
            
            if let Ok(o) = output {
                if o.status.success() {
                    restored_count += 1;
                }
            }
        }
    }
    
    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
    
    Ok(restored_count)
}

/// Restore Homebrew cache from backup
fn restore_homebrew_cache(backup_path: &Path, archive_name: &str) -> Result<usize, String> {
    let archive = backup_path.join(archive_name);
    let home = dirs::home_dir().ok_or("Home directory not found")?;
    
    // Homebrew cache location
    let cache_path = home.join("Library/Caches/Homebrew");
    fs::create_dir_all(&cache_path).map_err(|e| e.to_string())?;
    
    // Extract archive directly into cache path
    extract_archive_to(&archive, &cache_path)?;
    
    // Calculate restored size in MB
    let mut total_size: u64 = 0;
    if let Ok(entries) = fs::read_dir(&cache_path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                total_size += meta.len();
            }
        }
    }
    
    Ok((total_size / 1_048_576) as usize)
}

/// Parallel MAS app installation with up to 4 concurrent downloads
/// Provides ~60-80% time savings when installing many apps
fn restore_mas_apps(
    backup_path: &Path,
    archive_name: &str,
    _reinstall: bool,
    window: Option<&tauri::Window>,
) -> Result<usize, String> {
    let archive = backup_path.join(archive_name);
    
    let temp_dir = std::env::temp_dir().join("macos-backup-restore-mas");
    extract_archive_to(&archive, &temp_dir)?;
    
    let apps_file = temp_dir.join("mas_apps.txt");
    if !apps_file.exists() {
        return Err("App list not found".to_string());
    }
    
    // Get list of currently installed apps
    let installed_before = Command::new("/bin/zsh")
        .args(["-l", "-c", "mas list"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    
    let file_content = fs::read_to_string(&apps_file).map_err(|e| e.to_string())?;
    let mut apps_to_install: Vec<String> = Vec::new();
    
    for line in file_content.lines() {
        if line.is_empty() || !line.starts_with("mas ") { continue; }
        
        // Format: mas "App Name", id: 123456
        if let Some(id_part) = line.split("id: ").nth(1) {
            let app_id = id_part.trim();

            // SECURITY: Strictly validate the App Store ID to prevent shell
            // command injection via crafted backup metadata. Mac App Store IDs
            // are always positive integers (usually 9-10 digits).
            if app_id.is_empty()
                || app_id.len() > 20
                || !app_id.chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }

            // Check if already installed (always skip - reinstall makes no sense for MAS)
            if installed_before.contains(app_id) {
                continue;
            }

            apps_to_install.push(app_id.to_string());
        }
    }
    
    let _ = fs::remove_dir_all(&temp_dir);
    
    // If no apps need to be installed, return 0
    if apps_to_install.is_empty() {
        return Ok(0);
    }
    
    // Native parallel MAS installation (kein externes Terminal-Fenster mehr).
    // Wir starten bis zu MAX_PARALLEL_MAS `mas install`-Prozesse gleichzeitig
    // und emittieren Fortschritts-Events in die Hauptfenster-UI.
    const MAX_PARALLEL_MAS: usize = 4;
    const MAS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60); // 15 min pro App

    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    let total = apps_to_install.len();
    let done = std::sync::Arc::new(AtomicUsize::new(0));
    let failed = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let queue = std::sync::Arc::new(Mutex::new(apps_to_install.clone().into_iter()));

    if let Some(w) = window {
        let _ = w.emit("restore-log", format!("⚙️ Installiere {} MAS-Apps (parallel x{})...", total, MAX_PARALLEL_MAS));
    }

    let mut handles = Vec::with_capacity(MAX_PARALLEL_MAS);
    for _ in 0..MAX_PARALLEL_MAS.min(total) {
        let queue = queue.clone();
        let done = done.clone();
        let failed = failed.clone();
        let window_cloned: Option<tauri::Window> = window.cloned();

        let handle = std::thread::spawn(move || {
            loop {
                // Cancel-Check
                if BACKUP_CANCELLED.load(Ordering::Relaxed) {
                    return;
                }
                let next = { queue.lock().ok().and_then(|mut q| q.next()) };
                let Some(app_id) = next else { return; };

                // App-ID defensiv nochmals validieren (nur Ziffern)
                if !app_id.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(mut f) = failed.lock() {
                        f.push(app_id.clone());
                    }
                    continue;
                }

                let mut cmd = Command::new("/bin/zsh");
                cmd.args(["-l", "-c", &format!("mas install {}", app_id)]);
                let result = run_with_timeout(cmd, MAS_TIMEOUT);

                let ok = matches!(&result, Ok(o) if o.status.success());
                if !ok {
                    if let Ok(mut f) = failed.lock() {
                        f.push(app_id.clone());
                    }
                }
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(ref w) = window_cloned {
                    let msg = if ok {
                        format!("✅ MAS {}/{}: App {} installiert", n, total, app_id)
                    } else {
                        format!("⚠️ MAS {}/{}: App {} fehlgeschlagen", n, total, app_id)
                    };
                    let _ = w.emit("restore-log", msg);
                    let _ = w.emit("restore-progress", serde_json::json!({
                        "progress": null,
                        "message": format!("MAS {}/{}", n, total)
                    }));
                }
            }
        });
        handles.push(handle);
    }

    for h in handles {
        let _ = h.join();
    }

    let failed_ids = failed.lock().map(|f| f.clone()).unwrap_or_default();
    let installed_count = total.saturating_sub(failed_ids.len());

    if !failed_ids.is_empty() {
        if let Some(w) = window {
            let _ = w.emit(
                "restore-log",
                format!("⚠️ {} MAS-Apps konnten nicht installiert werden: {}",
                    failed_ids.len(),
                    failed_ids.join(", ")),
            );
        }
    }

    Ok(installed_count)
}


/// Parallel VS Code extension installation with up to 6 concurrent installs
/// Provides ~60-80% time savings when installing many extensions
fn restore_vscode_extensions(backup_path: &Path, archive_name: &str, _reinstall: bool) -> Result<usize, String> {
    let archive = backup_path.join(archive_name);
    
    let temp_dir = std::env::temp_dir().join("macos-backup-restore-vscode");
    extract_archive_to(&archive, &temp_dir)?;
    
    let ext_file = temp_dir.join("vscode_extensions.txt");
    if !ext_file.exists() {
        return Err("Extensions list not found".to_string());
    }
    
    let file_content = fs::read_to_string(&ext_file).map_err(|e| e.to_string())?;
    let extensions: Vec<&str> = file_content.lines().filter(|l| !l.is_empty()).collect();
    let total = extensions.len();
    
    if total == 0 {
        let _ = fs::remove_dir_all(&temp_dir);
        return Ok(0);
    }
    
    // Parallel VS Code extension installation with up to 6 concurrent installs
    const MAX_PARALLEL_VSCODE: usize = 6;
    
    // Use rayon for parallel processing if available, otherwise use threads
    let force_flag = if _reinstall { "--force" } else { "" };
    
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    
    let installed_counter = Arc::new(AtomicUsize::new(0));
    let extensions_owned: Vec<String> = extensions.iter().map(|s| s.to_string()).collect();
    
    // Process extensions in parallel batches
    let chunks: Vec<Vec<String>> = extensions_owned
        .chunks(MAX_PARALLEL_VSCODE)
        .map(|c| c.to_vec())
        .collect();
    
    for chunk in chunks {
        let mut batch_handles: Vec<std::thread::JoinHandle<()>> = Vec::new();
        
        for ext in chunk {
            let counter = Arc::clone(&installed_counter);
            let force = force_flag.to_string();
            
            let handle = std::thread::spawn(move || {
                let cmd = if force.is_empty() {
                    format!("code --install-extension {}", ext)
                } else {
                    format!("code --install-extension {} {}", ext, force)
                };
                
                let result = Command::new("/bin/zsh")
                    .args(["-l", "-c", &cmd])
                    .output();
                
                if let Ok(output) = result {
                    if output.status.success() {
                        counter.fetch_add(1, AtomicOrdering::SeqCst);
                    }
                }
            });
            
            batch_handles.push(handle);
        }
        
        // Wait for this batch to complete before starting next
        for handle in batch_handles {
            let _ = handle.join();
        }
    }
    
    let installed = installed_counter.load(AtomicOrdering::SeqCst);
    
    let _ = fs::remove_dir_all(&temp_dir);
    
    if installed == 0 && total > 0 {
        return Err(format!("No extensions installed (0/{})", total));
    }
    
    Ok(installed)
}

#[tauri::command]
fn delete_backup(target_path: String, timestamp: String) -> Result<(), String> {
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

        let (bytes, is_file) = if expanded.is_file() {
            (fs::metadata(&expanded).map(|m| m.len()).unwrap_or(0), true)
        } else {
            (compute_directory_size(&expanded), false)
        };
        total_bytes += bytes;

        let name = expanded.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "backup".to_string());
        let archive_ext = if is_zstd_available() { "tar.zst" } else { "tar.gz" };
        let archive_name = format!(
            "{}.{}",
            name.to_lowercase().replace(' ', "-").replace('.', "_"),
            archive_ext
        );

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
    let estimated_archive_bytes = total_bytes / 2;

    Ok(serde_json::json!({
        "target_path": target_path,
        "items": items,
        "missing": missing,
        "total_source_bytes": total_bytes,
        "estimated_archive_bytes": estimated_archive_bytes,
        "available_bytes": available_bytes,
        "sufficient_space": available_bytes as i128 > estimated_archive_bytes as i128,
        "zstd_available": is_zstd_available(),
    }))
}

// ========== Menu Building ==========

fn build_menu(app_handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let about_metadata = AboutMetadata {
        name: Some("macOS Backup Suite".to_string()),
        version: Some("1.0.0".to_string()),
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
    OPERATION_IN_PROGRESS.store(false, Ordering::SeqCst);
    
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
    BACKUP_CANCELLED.store(false, Ordering::SeqCst);
    VERIFY_CANCELLED.store(false, Ordering::SeqCst);
    OPERATION_IN_PROGRESS.store(true, Ordering::SeqCst);
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
