import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize, LogicalPosition } from "@tauri-apps/api/window";
import { open, save, ask } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { sendNotification } from "@tauri-apps/plugin-notification";

// Types
interface BackupConfig {
  target_volume: string;
  target_directory: string;
  directories: string[];
  backup_homebrew: boolean;
  backup_mas: boolean;
  default_directories: string[];
  language: string;
  theme: string;
}

interface BackupItem {
  timestamp: string;
  hash_verified: boolean;
}

interface BackupFileInfo {
  path: string;
  archive: string;
  archive_size_bytes: number;
  source_size_bytes: number;
}

interface BackupDetails {
  timestamp: string;
  items: BackupFileInfo[];
  total_source_size_bytes: number;
  total_archive_size_bytes: number;
  start_time: string;
  end_time: string;
  duration_seconds: number;
}

interface RestoreResult {
  restored_count: number;
  skipped_count: number;
  error_count: number;
  restored: string[];
  skipped: string[];
  errors: string[];
}

interface BackupFileInfo {
  path: string;
  archive: string;
  archive_size_bytes: number;
  source_size_bytes: number;
}

interface BackupDetails {
  timestamp: string;
  items: BackupFileInfo[];
  total_source_size_bytes: number;
  total_archive_size_bytes: number;
  start_time: string;
  end_time: string;
  duration_seconds: number;
}

interface RestoreResult {
  restored_count: number;
  skipped_count: number;
  error_count: number;
  restored: string[];
  skipped: string[];
  errors: string[];
}

interface Volume {
  name: string;
  path: string;
  available: boolean;
  writable: boolean;
  is_internal: boolean;
  free_space_gb: number;
}

interface UserFolder {
  name: string;
  path: string;
  readable: boolean;
  is_current_user: boolean;
}

interface PermissionCheckResult {
  path: string;
  readable: boolean;
  error_message: string | null;
}

interface FullDiskAccessStatus {
  has_full_disk_access: boolean;
  tested_paths: string[];
  inaccessible_paths: string[];
}

// Translations
const translations: Record<string, Record<string, string>> = {
  de: {
    ready: "Bereit",
    backupTarget: "Backup-Ziel",
    volume: "Volume:",
    pleaseSelect: "Bitte wählen...",
    targetDirectory: "Zielverzeichnis:",
    notSelected: "Nicht ausgewählt",
    foldersToBackup: "Zu sichernde Ordner",
    addFolder: "Ordner hinzufügen",
    addUserFolder: "Benutzerordner...",
    reset: "Zurücksetzen",
    actions: "Aktionen",
    createBackup: "Backup erstellen",
    cancel: "Abbrechen",
    restore: "Wiederherstellen",
    existingBackups: "Vorhandene Backups",
    deleteBackup: "Löschen",
    confirmDeleteBackup: "Backup wirklich löschen?",
    backupDeleted: "Backup gelöscht",
    deleteError: "Fehler beim Löschen:",
    selectBackup: "Backup wählen...",
    showFiles: "Dateien anzeigen",
    showManualApps: "Manuelle Apps",
    manualAppsTitle: "Manuell installierte Apps",
    manualAppsDescription: "Diese Apps wurden nicht über Homebrew oder den App Store installiert und müssen manuell heruntergeladen werden:",
    noManualApps: "Keine manuell installierten Apps gefunden.",
    manualAppsError: "Fehler beim Laden der manuellen Apps:",
    selectBackupForManualApps: "Bitte wählen Sie zuerst ein Backup aus!",
    verify: "Verifizieren",
    protocol: "Protokoll",
    copy: "Kopieren",
    save: "Speichern",
    delete: "Löschen",
    settings: "Einstellungen",
    defaultFolders: "Standard-Ordner",
    defaultFoldersHint: "Diese Ordner werden beim Zurücksetzen verwendet:",
    externalVolumes: "Externe Volumes",
    internalVolumes: "Interne Volumes",
    noBackups: "Keine Backups vorhanden",
    selectTargetFirst: "Bitte zuerst ein Ziel wählen",
    backupRunning: "Backup läuft...",
    startingBackup: "Starte Backup...",
    backupComplete: "Backup abgeschlossen!",
    backupFailed: "Backup fehlgeschlagen!",
    backupCancelled: "Backup abgebrochen!",
    configLoaded: "Konfiguration geladen.",
    defaultConfigUsed: "Standardkonfiguration verwendet.",
    volumesFound: "beschreibbare Volumes gefunden (Time Machine ausgeschlossen).",
    folderAdded: "Ordner hinzugefügt:",
    folderReset: "Ordnerliste auf Standardwerte zurückgesetzt.",
    selectError: "Fehler beim Auswählen:",
    copySuccess: "Protokoll in Zwischenablage kopiert.",
    copyError: "Fehler beim Kopieren:",
    saveSuccess: "Protokoll gespeichert:",
    saveError: "Fehler beim Speichern:",
    logCleared: "Protokoll gelöscht.",
    started: "macOS Backup Suite gestartet.",
    homebrewFound: "Homebrew gefunden.",
    homebrewNotInstalled: "Homebrew nicht installiert.",
    masFound: "Mac App Store CLI (mas) gefunden.",
    masNotInstalled: "Mac App Store CLI (mas) nicht installiert.",
    homebrewCheckFailed: "Homebrew-Check fehlgeschlagen.",
    backupNotification: "Backup abgeschlossen",
    backupNotificationBody: "Das Backup wurde erfolgreich erstellt.",
    selectRestoreBackup: "Bitte ein Backup zum Wiederherstellen auswählen!",
    restoreStarted: "Wiederherstellung von Backup gestartet...",
    restoreComingSoon: "Restore-Funktion wird in Kürze implementiert.",
    selectTestBackup: "Bitte ein Backup für den Test auswählen!",
    verifyStarted: "Hash-Verifizierung für Backup",
    verifyComingSoon: "Verifizierung wird in Kürze implementiert.",
    selectFilesBackup: "Bitte ein Backup auswählen!",
    filesLoading: "Dateiliste wird geladen...",
    filesComingSoon: "Dateiliste wird in Kürze implementiert.",
    selectVolumeFirst: "Bitte zuerst ein Volume auswählen.",
    backupTargetSet: "Backup-Ziel:",
    selectBackupTarget: "Bitte ein Backup-Ziel auswählen!",
    settingsSaved: "Einstellungen gespeichert.",
    freeSpace: "frei",
    selectUserFolder: "Benutzerordner auswählen",
    userFolders: "Benutzerordner",
    currentUser: "Aktueller Benutzer",
    otherUsers: "Andere Benutzer",
    noReadAccess: "Kein Lesezugriff",
    selectSubfolder: "Unterordner auswählen",
    close: "Schließen",
    noPermission: "Keine Leseberechtigung für diesen Ordner.",
    noPermissionHint: "Für Ordner anderer Benutzer ist Full Disk Access erforderlich.",
    fullDiskAccessRequired: "Full Disk Access erforderlich",
    fullDiskAccessHint: "Um Ordner anderer Benutzer zu sichern, aktiviere Full Disk Access in den Systemeinstellungen. Nach dem Aktivieren muss die App neu gestartet werden.",
    openSettings: "Einstellungen öffnen",
    restartApp: "App neu starten",
    fullDiskAccessGranted: "Full Disk Access ist aktiviert.",
    fullDiskAccessMissing: "⚠️ Eingeschränkter Zugriff – Full Disk Access fehlt.",
    checkingAccess: "Prüfe Zugriffsrechte...",
    permissionDenied: "Zugriff verweigert:",
    addSystemConfigs: "System-Configs",
    systemConfigsAdded: "System-Konfigurationspfade hinzugefügt:",
    systemConfigsHint: "Wichtige Konfig-Dateien für schnelle Wiederherstellung",
    restoreModalTitle: "Wiederherstellung",
    selectItemsToRestore: "Elemente zur Wiederherstellung auswählen:",
    overwriteExisting: "Bestehende Dateien überschreiben",
    overwriteHint: "Wenn deaktiviert, werden existierende Dateien übersprungen",
    startRestore: "Wiederherstellen",
    cancelRestore: "Abbrechen",
    selectAll: "Alle auswählen",
    deselectAll: "Alle abwählen",
    restoreComplete: "Wiederherstellung abgeschlossen",
    restoredItems: "Wiederhergestellt",
    skippedItems: "Übersprungen",
    errorItems: "Fehler",
    restoring: "Wiederherstellen von",
    noItemsSelected: "Keine Elemente ausgewählt!",
  },
  en: {
    ready: "Ready",
    backupTarget: "Backup Target",
    volume: "Volume:",
    pleaseSelect: "Please select...",
    targetDirectory: "Target Directory:",
    notSelected: "Not selected",
    foldersToBackup: "Folders to Backup",
    addFolder: "Add Folder",
    addUserFolder: "User Folder...",
    reset: "Reset",
    actions: "Actions",
    createBackup: "Create Backup",
    cancel: "Cancel",
    restore: "Restore",
    existingBackups: "Existing Backups",
    deleteBackup: "Delete",
    confirmDeleteBackup: "Really delete backup?",
    backupDeleted: "Backup deleted",
    deleteError: "Error deleting:",
    selectBackup: "Select backup...",
    showFiles: "Show Files",
    showManualApps: "Manual Apps",
    manualAppsTitle: "Manually Installed Apps",
    manualAppsDescription: "These apps were not installed via Homebrew or App Store and need to be downloaded manually:",
    noManualApps: "No manually installed apps found.",
    manualAppsError: "Error loading manual apps:",
    selectBackupForManualApps: "Please select a backup first!",
    verify: "Verify",
    protocol: "Log",
    copy: "Copy",
    save: "Save",
    delete: "Delete",
    settings: "Settings",
    defaultFolders: "Default Folders",
    defaultFoldersHint: "These folders are used when resetting:",
    externalVolumes: "External Volumes",
    internalVolumes: "Internal Volumes",
    noBackups: "No backups available",
    selectTargetFirst: "Please select a target first",
    backupRunning: "Backup running...",
    startingBackup: "Starting backup...",
    backupComplete: "Backup complete!",
    backupFailed: "Backup failed!",
    backupCancelled: "Backup cancelled!",
    configLoaded: "Configuration loaded.",
    defaultConfigUsed: "Default configuration used.",
    volumesFound: "writable volumes found (Time Machine excluded).",
    folderAdded: "Folder added:",
    folderReset: "Folder list reset to default.",
    selectError: "Selection error:",
    copySuccess: "Log copied to clipboard.",
    copyError: "Copy error:",
    saveSuccess: "Log saved:",
    saveError: "Save error:",
    logCleared: "Log cleared.",
    started: "macOS Backup Suite started.",
    homebrewFound: "Homebrew found.",
    homebrewNotInstalled: "Homebrew not installed.",
    masFound: "Mac App Store CLI (mas) found.",
    masNotInstalled: "Mac App Store CLI (mas) not installed.",
    homebrewCheckFailed: "Homebrew check failed.",
    backupNotification: "Backup Complete",
    backupNotificationBody: "The backup was successfully created.",
    selectRestoreBackup: "Please select a backup to restore!",
    restoreStarted: "Restore from backup started...",
    restoreComingSoon: "Restore function coming soon.",
    selectTestBackup: "Please select a backup for testing!",
    verifyStarted: "Hash verification for backup",
    verifyComingSoon: "Verification coming soon.",
    selectFilesBackup: "Please select a backup!",
    filesLoading: "Loading file list...",
    filesComingSoon: "File list coming soon.",
    selectVolumeFirst: "Please select a volume first.",
    backupTargetSet: "Backup target:",
    selectBackupTarget: "Please select a backup target!",
    settingsSaved: "Settings saved.",
    freeSpace: "free",
    selectUserFolder: "Select User Folder",
    userFolders: "User Folders",
    currentUser: "Current User",
    otherUsers: "Other Users",
    noReadAccess: "No read access",
    selectSubfolder: "Select Subfolder",
    close: "Close",
    noPermission: "No read permission for this folder.",
    noPermissionHint: "Full Disk Access is required for other users' folders.",
    fullDiskAccessRequired: "Full Disk Access Required",
    fullDiskAccessHint: "To backup folders of other users, enable Full Disk Access in System Settings. After enabling, the app must be restarted.",
    openSettings: "Open Settings",
    restartApp: "Restart App",
    fullDiskAccessGranted: "Full Disk Access is enabled.",
    fullDiskAccessMissing: "⚠️ Limited access – Full Disk Access missing.",
    checkingAccess: "Checking access rights...",
    permissionDenied: "Access denied:",
    addSystemConfigs: "System Configs",
    systemConfigsAdded: "System config paths added:",
    systemConfigsHint: "Important config files for quick restore",
  }
};

// Current language
let currentLanguage = "de";

function t(key: string): string {
  return translations[currentLanguage]?.[key] || translations.de[key] || key;
}

// DOM Elements
const volumeSelect = document.getElementById("volume-select") as HTMLSelectElement;
const refreshVolumesBtn = document.getElementById("refresh-volumes") as HTMLButtonElement;
const targetPathDisplay = document.getElementById("target-path-display") as HTMLSpanElement;
const browseTargetBtn = document.getElementById("browse-target") as HTMLButtonElement;
const directoriesList = document.getElementById("directories-list") as HTMLUListElement;
const addDirectoryBtn = document.getElementById("add-directory") as HTMLButtonElement;
const addUserDirectoryBtn = document.getElementById("add-user-directory") as HTMLButtonElement;
const addSystemConfigsBtn = document.getElementById("add-system-configs") as HTMLButtonElement;
const resetDirectoriesBtn = document.getElementById("reset-directories") as HTMLButtonElement;
const btnBackup = document.getElementById("btn-backup") as HTMLButtonElement;
const btnCancel = document.getElementById("btn-cancel") as HTMLButtonElement;
const btnRestore = document.getElementById("btn-restore") as HTMLButtonElement;
const btnRestoreTest = document.getElementById("btn-restore-test") as HTMLButtonElement;
const backupSelect = document.getElementById("backup-select") as HTMLSelectElement;
const showFilesBtn = document.getElementById("show-files") as HTMLButtonElement;
const showManualAppsBtn = document.getElementById("show-manual-apps") as HTMLButtonElement;
const btnDeleteBackup = document.getElementById("btn-delete-backup") as HTMLButtonElement;
const restoreModal = document.getElementById("restore-modal") as HTMLDivElement;
const restoreItemsList = document.getElementById("restore-items-list") as HTMLDivElement;
const restoreSelectAll = document.getElementById("restore-select-all") as HTMLButtonElement;
const restoreDeselectAll = document.getElementById("restore-deselect-all") as HTMLButtonElement;
const restoreOverwrite = document.getElementById("restore-overwrite") as HTMLInputElement;
const restoreCancel = document.getElementById("restore-cancel") as HTMLButtonElement;
const restoreStart = document.getElementById("restore-start") as HTMLButtonElement;
const progressMessage = document.getElementById("progress-message") as HTMLParagraphElement;
const progressFill = document.getElementById("progress-fill") as HTMLDivElement;
const logOutput = document.getElementById("log-output") as HTMLPreElement;
const copyLogBtn = document.getElementById("copy-log") as HTMLButtonElement;
const saveLogBtn = document.getElementById("save-log") as HTMLButtonElement;
const clearLogBtn = document.getElementById("clear-log") as HTMLButtonElement;
const statusEl = document.getElementById("status") as HTMLParagraphElement;
const btnSettings = document.getElementById("btn-settings") as HTMLButtonElement;
const btnLanguage = document.getElementById("btn-language") as HTMLButtonElement;
const btnTheme = document.getElementById("btn-theme") as HTMLButtonElement;
const settingsDialog = document.getElementById("settings-dialog") as HTMLDialogElement;
const defaultDirectoriesList = document.getElementById("default-directories-list") as HTMLUListElement;
const addDefaultDirectoryBtn = document.getElementById("add-default-directory") as HTMLButtonElement;
const settingsCancelBtn = document.getElementById("settings-cancel") as HTMLButtonElement;
const settingsSaveBtn = document.getElementById("settings-save") as HTMLButtonElement;
const userFolderDialog = document.getElementById("user-folder-dialog") as HTMLDialogElement;
const userFolderList = document.getElementById("user-folder-list") as HTMLUListElement;
const userFolderCloseBtn = document.getElementById("user-folder-close") as HTMLButtonElement;
const fdaWarning = document.getElementById("fda-warning") as HTMLDivElement;

// State
let config: BackupConfig = {
  target_volume: "",
  target_directory: "",
  directories: [],
  backup_homebrew: true,
  backup_mas: true,
  default_directories: [],
  language: "de",
  theme: "auto",
};

let currentVolumes: Volume[] = [];
let backupInProgress = false;
let tempDefaultDirectories: string[] = [];
let hasFDA = true; // Full Disk Access status
let fdaMessageShown = false; // Track if FDA message was already shown

const INITIAL_DEFAULT_DIRECTORIES = [
  "~/Documents",
  "~/Desktop", 
  "~/Downloads",
  "~/Music",
  "~/Pictures",
  "~/.ssh",
  "~/.gitconfig",
  "~/.zshrc",
];
// System configuration directories for quick restore after OS reinstall
const SYSTEM_CONFIG_DIRECTORIES = [
  // Developer configs
  "~/.ssh",
  "~/.gitconfig",
  "~/.zshrc",
  "~/.bashrc",
  "~/.bash_profile",
  "~/.config",
  "~/.gnupg",
  // Package managers
  "~/.npm",
  "~/.nvm",
  "~/.pyenv",
  "~/.conda",
  "~/.cargo",
  // App settings
  "~/Library/Preferences",
  "~/Library/Application Support/Code/User",
  "~/Library/Application Support/JetBrains",
  "~/Library/Keychains",
  "~/Library/Services",
  "~/Library/Fonts",
  "~/Library/ColorSync/Profiles",
  "~/Library/Keyboard Layouts",
];

// System configuration directories for quick restore after OS reinstall


// Helpers
function formatBytes(gb: number): string {
  if (gb >= 1000) {
    return `${(gb / 1000).toFixed(1)} TB`;
  }
  return `${gb.toFixed(1)} GB`;
}

// Theme management
function applyTheme(theme: string): void {
  const root = document.documentElement;
  root.classList.remove("light-theme", "dark-theme");
  
  if (theme === "light") {
    root.classList.add("light-theme");
    btnTheme.textContent = "☀️";
  } else if (theme === "dark") {
    root.classList.add("dark-theme");
    btnTheme.textContent = "🌙";
  } else {
    btnTheme.textContent = "🌓";
  }
  
  config.theme = theme;
}

function cycleTheme(): void {
  if (config.theme === "auto") {
    applyTheme("light");
  } else if (config.theme === "light") {
    applyTheme("dark");
  } else {
    applyTheme("auto");
  }
  saveConfig();
}

// Language management
function applyLanguage(lang: string): void {
  currentLanguage = lang;
  config.language = lang;
  btnLanguage.textContent = lang === "de" ? "🇩🇪" : "🇬🇧";
  updateUITranslations();
}

function toggleLanguage(): void {
  const newLang = currentLanguage === "de" ? "en" : "de";
  applyLanguage(newLang);
  saveConfig();
  log(currentLanguage === "de" ? "Sprache: Deutsch" : "Language: English");
}

function updateUITranslations(): void {
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    const key = el.getAttribute("data-i18n")!;
    el.textContent = t(key);
  });
  
  statusEl.textContent = t("ready");
  
  const sections = document.querySelectorAll(".section h2");
  const sectionKeys = ["backupTarget", "foldersToBackup", "actions", "existingBackups", "protocol"];
  const emojis = ["📁", "📂", "🚀", "🔄", "📜"];
  sections.forEach((section, i) => {
    if (sectionKeys[i]) {
      section.textContent = `${emojis[i]} ${t(sectionKeys[i])}`;
    }
  });
  
  btnBackup.innerHTML = `📤 ${t("createBackup")}`;
  btnCancel.textContent = t("cancel");
  btnRestore.innerHTML = `📥 ${t("restore")}`;
  addDirectoryBtn.innerHTML = `+ ${t("addFolder")}`;
  if (addUserDirectoryBtn) {
    addUserDirectoryBtn.innerHTML = `👤 ${t("addUserFolder")}`;
  }
  resetDirectoriesBtn.innerHTML = `↻ ${t("reset")}`;
  showFilesBtn.innerHTML = `📋 ${t("showFiles")}`;
  btnRestoreTest.innerHTML = `✓ ${t("verify")}`;
  btnDeleteBackup.innerHTML = `🗑️ ${t("deleteBackup")}`;
  btnDeleteBackup.innerHTML = `🗑️ ${t("deleteBackup")}`;
  copyLogBtn.innerHTML = `📋 ${t("copy")}`;
  saveLogBtn.innerHTML = `💾 ${t("save")}`;
  clearLogBtn.innerHTML = `🗑️ ${t("delete")}`;
  
  const labels = document.querySelectorAll(".form-group label");
  if (labels[0]) labels[0].textContent = t("volume");
  if (labels[1]) labels[1].textContent = t("targetDirectory");
  
  if (currentVolumes.length > 0) {
    updateVolumeSelect();
  }
  
  updateBackupSelectPlaceholder();
  updateFDAWarning();
}

// Logging
function log(message: string): void {
  const timestamp = new Date().toLocaleTimeString(currentLanguage === "de" ? "de-DE" : "en-US");
  logOutput.textContent += `[${timestamp}] ${message}\n`;
  logOutput.scrollTop = logOutput.scrollHeight;
}

// Load config from backend
async function loadConfig(): Promise<void> {
  try {
    config = await invoke<BackupConfig>("load_config");
    if (!config.default_directories || config.default_directories.length === 0) {
      config.default_directories = [...INITIAL_DEFAULT_DIRECTORIES];
    }
    if (config.directories.length === 0) {
      config.directories = [...config.default_directories];
    }
    if (config.language) {
      applyLanguage(config.language);
    }
    if (config.theme) {
      applyTheme(config.theme);
    }
    updateDirectoriesList();
    updateTargetPathDisplay();
    log(t("configLoaded"));
  } catch (e) {
    config.default_directories = [...INITIAL_DEFAULT_DIRECTORIES];
    config.directories = [...INITIAL_DEFAULT_DIRECTORIES];
    updateDirectoriesList();
    log(t("defaultConfigUsed"));
  }
}

// Save config to backend
async function saveConfig(): Promise<void> {
  try {
    await invoke("save_config", { config });
  } catch (e) {
    log(`${t("saveError")} ${e}`);
  }
}

// Update target path display
function updateTargetPathDisplay(): void {
  const fullPath = getFullTargetPath();
  if (targetPathDisplay) {
    targetPathDisplay.textContent = fullPath || t("notSelected");
    targetPathDisplay.title = fullPath;
  }
}

// Get full target path (volume + directory)
function getFullTargetPath(): string {
  if (!config.target_volume) return "";
  if (config.target_directory) {
    return `${config.target_volume}/${config.target_directory}`;
  }
  return config.target_volume;
}

// Update volume select with current volumes
function updateVolumeSelect(): void {
  volumeSelect.innerHTML = `<option value="">${t("pleaseSelect")}</option>`;
  
  const external = currentVolumes.filter(v => !v.is_internal);
  const internal = currentVolumes.filter(v => v.is_internal);
  
  if (external.length > 0) {
    const extGroup = document.createElement("optgroup");
    extGroup.label = t("externalVolumes");
    for (const vol of external) {
      const option = document.createElement("option");
      option.value = vol.path;
      option.textContent = `${vol.name} (${formatBytes(vol.free_space_gb)} ${t("freeSpace")})`;
      extGroup.appendChild(option);
    }
    volumeSelect.appendChild(extGroup);
  }
  
  if (internal.length > 0) {
    const intGroup = document.createElement("optgroup");
    intGroup.label = t("internalVolumes");
    for (const vol of internal) {
      const option = document.createElement("option");
      option.value = vol.path;
      option.textContent = `${vol.name} (${formatBytes(vol.free_space_gb)} ${t("freeSpace")})`;
      intGroup.appendChild(option);
    }
    volumeSelect.appendChild(intGroup);
  }
  
  if (config.target_volume) {
    volumeSelect.value = config.target_volume;
  }
}

// Update backup select placeholder
function updateBackupSelectPlaceholder(): void {
  const targetPath = getFullTargetPath();
  if (!targetPath) {
    backupSelect.innerHTML = `<option value="">${t("selectTargetFirst")}</option>`;
  } else if (backupSelect.options.length <= 1) {
    const firstOption = backupSelect.options[0];
    if (firstOption) {
      if (firstOption.value === "") {
        firstOption.textContent = t("selectBackup");
      }
    }
  }
}

// Load external volumes
async function loadVolumes(): Promise<void> {
  try {
    currentVolumes = await invoke<Volume[]>("get_external_volumes");
    updateVolumeSelect();
    log(`${currentVolumes.length} ${t("volumesFound")}`);
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
}

// Load available backups
async function loadBackups(): Promise<void> {
  const targetPath = getFullTargetPath();
  if (!targetPath) {
    backupSelect.innerHTML = `<option value="">${t("selectTargetFirst")}</option>`;
    return;
  }
  
  try {
    const backups = await invoke<BackupItem[]>("list_backups", {
      targetPath: targetPath,
    });
    
    backupSelect.innerHTML = `<option value="">${t("selectBackup")}</option>`;
    for (const backup of backups) {
      const option = document.createElement("option");
      option.value = backup.timestamp;
      const verified = backup.hash_verified ? "✓" : "✗";
      const formatted = formatTimestamp(backup.timestamp);
      option.textContent = `${formatted} [${verified}]`;
      backupSelect.appendChild(option);
    }
    
    if (backups.length === 0) {
      backupSelect.innerHTML = `<option value="">${t("noBackups")}</option>`;
    }
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
}

// Format timestamp from YYYYMMDD-HHMMSS to readable format
function formatTimestamp(ts: string): string {
  if (ts.length !== 15) return ts;
  const year = ts.substring(0, 4);
  const month = ts.substring(4, 6);
  const day = ts.substring(6, 8);
  const hour = ts.substring(9, 11);
  const min = ts.substring(11, 13);
  return `${day}.${month}.${year} ${hour}:${min}`;
}

// Update directories list UI
function updateDirectoriesList(): void {
  directoriesList.innerHTML = "";
  for (const dir of config.directories) {
    const li = document.createElement("li");
    li.innerHTML = `
      <span>${dir}</span>
      <button class="remove-dir" data-path="${dir}">✕</button>
    `;
    directoriesList.appendChild(li);
  }
  
  document.querySelectorAll(".remove-dir").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      const path = (e.target as HTMLButtonElement).dataset.path!;
      config.directories = config.directories.filter((d) => d !== path);
      updateDirectoriesList();
      saveConfig();
    });
  });
}

// Update default directories list in settings
function updateDefaultDirectoriesList(): void {
  defaultDirectoriesList.innerHTML = "";
  for (const dir of tempDefaultDirectories) {
    const li = document.createElement("li");
    li.innerHTML = `
      <span>${dir}</span>
      <button class="remove-dir" data-path="${dir}">✕</button>
    `;
    defaultDirectoriesList.appendChild(li);
  }
  
  defaultDirectoriesList.querySelectorAll(".remove-dir").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      const path = (e.target as HTMLButtonElement).dataset.path!;
      tempDefaultDirectories = tempDefaultDirectories.filter((d) => d !== path);
      updateDefaultDirectoriesList();
    });
  });
}

// Check Full Disk Access status
async function checkFullDiskAccess(): Promise<void> {
  try {
    const status = await invoke<FullDiskAccessStatus>("check_full_disk_access");
    hasFDA = status.has_full_disk_access;
    
    if (hasFDA) {
      if (!fdaMessageShown) {
        log(t("fullDiskAccessGranted"));
        fdaMessageShown = true;
      }
    } else {
      log(t("fullDiskAccessMissing"));
    }
    
    updateFDAWarning();
  } catch (e) {
    log(`${t("checkingAccess")} ${e}`);
  }
}

// Update FDA warning visibility
function updateFDAWarning(): void {
  if (fdaWarning) {
    if (!hasFDA) {
      fdaWarning.style.display = "block";
      fdaWarning.innerHTML = `
        <div class="warning-content">
          <span class="warning-icon">⚠️</span>
          <div class="warning-text">
            <strong>${t("fullDiskAccessRequired")}</strong>
            <p>${t("fullDiskAccessHint")}</p>
          </div>
          <div class="warning-buttons">
            <button id="fda-open-settings" class="btn-secondary btn-small">${t("openSettings")}</button>
            <button id="fda-restart-app" class="btn-primary btn-small">${t("restartApp")}</button>
          </div>
        </div>
      `;
      
      const openSettingsBtn = document.getElementById("fda-open-settings");
      if (openSettingsBtn) {
        openSettingsBtn.addEventListener("click", openPrivacySettings);
      }
      const restartBtn = document.getElementById("fda-restart-app");
      if (restartBtn) {
        restartBtn.addEventListener("click", restartApp);
      }
    } else {
      fdaWarning.style.display = "none";
    }
  }
}

// Open macOS Privacy Settings
async function openPrivacySettings(): Promise<void> {
  try {
    await invoke("open_privacy_settings");
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
}

// Restart the app to pick up new FDA permissions
async function restartApp(): Promise<void> {
  try {
    await invoke("restart_app");
  } catch (e) {
    log(`Restart error: ${e}`);
  }
}

// Show user folder picker dialog
async function showUserFolderPicker(): Promise<void> {
  try {
    const userFolders = await invoke<UserFolder[]>("list_user_folders");
    
    userFolderList.innerHTML = "";
    
    // Group by current user / others
    const currentUserFolders = userFolders.filter(u => u.is_current_user);
    const otherUserFolders = userFolders.filter(u => !u.is_current_user);
    
    // Current user section
    if (currentUserFolders.length > 0) {
      const header = document.createElement("li");
      header.className = "user-folder-header";
      header.textContent = `👤 ${t("currentUser")}`;
      userFolderList.appendChild(header);
      
      for (const user of currentUserFolders) {
        addUserFolderItem(user);
      }
    }
    
    // Other users section
    if (otherUserFolders.length > 0) {
      const header = document.createElement("li");
      header.className = "user-folder-header";
      header.textContent = `👥 ${t("otherUsers")}`;
      userFolderList.appendChild(header);
      
      for (const user of otherUserFolders) {
        addUserFolderItem(user);
      }
    }
    
    userFolderDialog.showModal();
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
}

// Add user folder item to the list
function addUserFolderItem(user: UserFolder): void {
  const li = document.createElement("li");
  li.className = `user-folder-item ${!user.readable ? 'no-access' : ''}`;
  
  const icon = user.is_current_user ? "🏠" : "👤";
  const accessIcon = user.readable ? "✓" : "🔒";
  const accessClass = user.readable ? "access-ok" : "access-denied";
  
  li.innerHTML = `
    <div class="user-folder-info">
      <span class="user-folder-icon">${icon}</span>
      <span class="user-folder-name">${user.name}</span>
      <span class="user-folder-access ${accessClass}">${accessIcon}</span>
    </div>
    <div class="user-folder-actions">
      <button class="btn-secondary btn-small select-user-home" data-path="${user.path}" ${!user.readable ? 'disabled' : ''}>
        ${t("addFolder")}
      </button>
      <button class="btn-secondary btn-small select-user-subfolder" data-path="${user.path}" ${!user.readable ? 'disabled' : ''}>
        ${t("selectSubfolder")}
      </button>
    </div>
  `;
  
  userFolderList.appendChild(li);
  
  // Event listeners
  const selectHomeBtn = li.querySelector(".select-user-home") as HTMLButtonElement;
  const selectSubfolderBtn = li.querySelector(".select-user-subfolder") as HTMLButtonElement;
  
  if (selectHomeBtn && user.readable) {
    selectHomeBtn.addEventListener("click", async () => {
      await addFolderWithPermissionCheck(user.path);
      userFolderDialog.close();
    });
  }
  
  if (selectSubfolderBtn && user.readable) {
    selectSubfolderBtn.addEventListener("click", async () => {
      userFolderDialog.close();
      await selectSubfolderFromUser(user.path);
    });
  }
}

// Select subfolder from a user's home directory
async function selectSubfolderFromUser(basePath: string): Promise<void> {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      defaultPath: basePath,
      title: t("selectSubfolder"),
    });
    
    if (selected) {
      await addFolderWithPermissionCheck(selected as string);
    }
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
}

// Add folder with permission check
async function addFolderWithPermissionCheck(path: string): Promise<void> {
  try {
    const result = await invoke<PermissionCheckResult>("check_read_permission", { path });
    
    if (!result.readable) {
      log(`${t("permissionDenied")} ${path}`);
      if (result.error_message) {
        log(`  → ${result.error_message}`);
      }
      log(t("noPermissionHint"));
      return;
    }
    
    // Convert to ~ path if in current user's home directory
    const homeDir = await invoke<string>("get_home_dir");
    let displayPath = path;
    if (path.startsWith(homeDir)) {
      displayPath = "~" + path.substring(homeDir.length);
    }
    
    if (!config.directories.includes(displayPath) && !config.directories.includes(path)) {
      config.directories.push(displayPath);
      updateDirectoriesList();
      await saveConfig();
      log(`${t("folderAdded")} ${displayPath}`);
    }
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
}

// Start backup
async function startBackup(): Promise<void> {
  // Re-check FDA before starting backup
  await checkFullDiskAccess();
  if (!hasFDA) {
    log(t("fullDiskAccessMissing"));
    statusEl.textContent = t("fullDiskAccessRequired");
    return;
  }

  const targetPath = getFullTargetPath();
  if (!targetPath) {
    log(`${t("selectBackupTarget")}`);
    return;
  }
  
  backupInProgress = true;
  btnBackup.disabled = true;
  btnBackup.style.display = "none";
  btnCancel.style.display = "block";
  statusEl.textContent = t("backupRunning");
  progressMessage.textContent = t("startingBackup");
  progressFill.style.width = "0%";
  
  try {
    await invoke("create_backup", {
      targetPath: targetPath,
      directories: config.directories,
    });
    
    if (backupInProgress) {
      await sendNotification({
        title: t("backupNotification"),
        body: t("backupNotificationBody"),
      });
      
      statusEl.textContent = t("backupComplete");
    }
    await loadBackups();
  } catch (e) {
    if (backupInProgress) {
      log(`${t("backupFailed")} ${e}`);
      statusEl.textContent = t("backupFailed");
    } else {
      statusEl.textContent = t("backupCancelled");
    }
  } finally {
    backupInProgress = false;
    btnBackup.disabled = false;
    btnBackup.style.display = "block";
    btnCancel.style.display = "none";
  }
}

// Cancel backup
async function cancelBackup(): Promise<void> {
  if (!backupInProgress) return;
  
  backupInProgress = false;
  try {
    await invoke("cancel_backup");
    log(t("backupCancelled"));
    // Reset UI state
    progressFill.style.width = "0%";
    progressMessage.textContent = t("backupCancelled");
    btnBackup.disabled = false;
    btnCancel.disabled = true;
  } catch (e) {
    log(`${t("backupFailed")} ${e}`);
  }
}

// Event listeners for progress updates from backend
async function setupEventListeners(): Promise<void> {
  await listen<string>("backup-log", (event) => {
    log(event.payload);
  });
  
  await listen<{ progress: number; message: string }>("backup-progress", (event) => {
    progressMessage.textContent = event.payload.message;
    progressFill.style.width = `${event.payload.progress}%`;
  });
}

// Event handlers
volumeSelect.addEventListener("change", async () => {
  config.target_volume = volumeSelect.value;
  config.target_directory = "";
  updateTargetPathDisplay();
  await saveConfig();
  await loadBackups();
});

refreshVolumesBtn.addEventListener("click", () => {
  loadVolumes();
});

browseTargetBtn?.addEventListener("click", async () => {
  if (!config.target_volume) {
    log(t("selectVolumeFirst"));
    return;
  }
  
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      defaultPath: config.target_volume,
      title: t("targetDirectory"),
    });
    
    if (selected) {
      const selectedPath = selected as string;
      if (selectedPath.startsWith(config.target_volume)) {
        const relativePath = selectedPath.substring(config.target_volume.length);
        config.target_directory = relativePath.replace(/^\//, "");
      } else {
        config.target_volume = selectedPath;
        config.target_directory = "";
        const matchedVol = currentVolumes.find(v => selectedPath.startsWith(v.path));
        if (matchedVol) {
          config.target_volume = matchedVol.path;
          const relativePath = selectedPath.substring(matchedVol.path.length);
          config.target_directory = relativePath.replace(/^\//, "");
          volumeSelect.value = matchedVol.path;
        }
      }
      updateTargetPathDisplay();
      await saveConfig();
      await loadBackups();
      log(`${t("backupTargetSet")} ${getFullTargetPath()}`);
    }
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
});

addDirectoryBtn.addEventListener("click", async () => {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: t("addFolder"),
    });
    
    if (selected) {
      await addFolderWithPermissionCheck(selected as string);
    }
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
});

if (addUserDirectoryBtn) {
  addUserDirectoryBtn.addEventListener("click", showUserFolderPicker);
}

// Add system configuration directories
async function addSystemConfigDirectories(): Promise<void> {
  let addedCount = 0;
  const homeDir = await invoke<string>("get_home_dir");
  
  for (const configPath of SYSTEM_CONFIG_DIRECTORIES) {
    // Expand ~ to check if path exists
    const expandedPath = configPath.startsWith("~/") 
      ? homeDir + configPath.substring(1)
      : configPath;
    
    // Check if already in list
    if (config.directories.includes(configPath)) {
      continue;
    }
    
    // Check if path exists and is readable
    try {
      const result = await invoke<PermissionCheckResult>("check_read_permission", { path: expandedPath });
      if (result.readable) {
        config.directories.push(configPath);
        addedCount++;
      }
    } catch (_e) {
      // Skip paths that don't exist or aren't accessible
    }
  }
  
  if (addedCount > 0) {
    updateDirectoriesList();
    await saveConfig();
    log(t("systemConfigsAdded") + " " + addedCount + " " + t("foldersToBackup"));
  } else {
    log(t("systemConfigsHint"));
  }
}

if (addSystemConfigsBtn) {
  addSystemConfigsBtn.addEventListener("click", addSystemConfigDirectories);
}

resetDirectoriesBtn.addEventListener("click", () => {
  config.directories = [...config.default_directories];
  updateDirectoriesList();
  saveConfig();
  log(t("folderReset"));
});

btnBackup.addEventListener("click", startBackup);

btnCancel.addEventListener("click", cancelBackup);

btnRestore.addEventListener("click", async () => {
  const timestamp = backupSelect.value;
  if (!timestamp) {
    log(t("selectRestoreBackup"));
    return;
  }
  
  const targetPath = getFullTargetPath();
  if (!targetPath) {
    log(t("selectTargetFirst"));
    return;
  }
  
  try {
    const details = await invoke<BackupDetails>("list_backup_files", {
      targetPath: targetPath,
      timestamp: timestamp,
    });
    showRestoreModal(details);
  } catch (e) {
    log(`❌ Fehler beim Laden der Backup-Details: ${e}`);
  }
});

function showRestoreModal(details: BackupDetails): void {
  document.getElementById("restore-modal-title")!.textContent = `🔄 ${t("restoreModalTitle")} - ${formatTimestamp(details.timestamp)}`;
  document.getElementById("restore-select-text")!.textContent = t("selectItemsToRestore");
  document.getElementById("overwrite-label")!.textContent = t("overwriteExisting");
  document.getElementById("overwrite-hint")!.textContent = t("overwriteHint");
  restoreSelectAll.textContent = t("selectAll");
  restoreDeselectAll.textContent = t("deselectAll");
  restoreCancel.textContent = t("cancelRestore");
  restoreStart.textContent = `🔄 ${t("startRestore")}`;
  
  restoreItemsList.innerHTML = "";
  for (const item of details.items) {
    const icon = getRestoreItemIcon(item.path);
    const size = formatRestoreBytes(item.source_size_bytes);
    const div = document.createElement("div");
    div.className = "restore-item";
    div.innerHTML = `
      <input type="checkbox" class="restore-checkbox" value="${item.path}" checked />
      <span class="restore-item-icon">${icon}</span>
      <div class="restore-item-info">
        <div class="restore-item-path">${item.path}</div>
        <div class="restore-item-size">${size}</div>
      </div>
    `;
    restoreItemsList.appendChild(div);
  }
  restoreModal.style.display = "flex";
}

function getRestoreItemIcon(path: string): string {
  if (path === "homebrew-packages") return "🍺";
  if (path === "mas-apps") return "🛒";
  if (path === "vscode-extensions") return "💻";
  if (path.includes("ssh")) return "🔑";
  if (path.includes("config")) return "⚙️";
  if (path.includes("Documents")) return "📄";
  if (path.includes("Desktop")) return "🖥️";
  if (path.includes("Pictures")) return "🖼️";
  if (path.includes("Music")) return "🎵";
  if (path.includes("Downloads")) return "📥";
  return "📁";
}

function formatRestoreBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

restoreSelectAll.addEventListener("click", () => {
  restoreItemsList.querySelectorAll<HTMLInputElement>(".restore-checkbox").forEach(cb => cb.checked = true);
});

restoreDeselectAll.addEventListener("click", () => {
  restoreItemsList.querySelectorAll<HTMLInputElement>(".restore-checkbox").forEach(cb => cb.checked = false);
});

restoreCancel.addEventListener("click", () => {
  restoreModal.style.display = "none";
});

restoreStart.addEventListener("click", async () => {
  const selectedItems: string[] = [];
  restoreItemsList.querySelectorAll<HTMLInputElement>(".restore-checkbox:checked").forEach(cb => {
    selectedItems.push(cb.value);
  });
  
  if (selectedItems.length === 0) {
    log(t("noItemsSelected"));
    return;
  }
  
  const overwrite = restoreOverwrite.checked;
  const timestamp = backupSelect.value;
  const targetPath = getFullTargetPath();
  
  restoreModal.style.display = "none";
  
  // Reset progress bar and start animation
  progressFill.style.width = "0%";
  progressFill.classList.add("animating");
  progressMessage.textContent = "Bereite Wiederherstellung vor...";
  
  log(`🔄 ${t("restoring")} ${selectedItems.length} Elemente...`);
  
  try {
    const result = await invoke<RestoreResult>("restore_items", {
      targetPath: targetPath,
      timestamp: timestamp,
      items: selectedItems,
      overwrite: overwrite,
    });
    
    log(`✅ ${t("restoreComplete")}:`);
    log(`   ${t("restoredItems")}: ${result.restored_count}`);
    if (result.skipped_count > 0) {
      log(`   ${t("skippedItems")}: ${result.skipped_count}`);
    }
    if (result.error_count > 0) {
      log(`   ${t("errorItems")}: ${result.error_count}`);
      for (const err of result.errors) {
        log(`   ❌ ${err}`);
      }
    }
    progressFill.classList.remove("animating");
    progressFill.style.width = "100%";
    progressMessage.textContent = t("restoreComplete");
  } catch (e) {
    log(`❌ Restore-Fehler: ${e}`);
    progressFill.classList.remove("animating");
    progressMessage.textContent = "Fehler bei Wiederherstellung";
  }
});

listen("restore-log", (event: { payload: string }) => {
  log(event.payload);
});

listen("restore-progress", (event: { payload: { progress: number; message: string } }) => {
  progressFill.style.width = `${event.payload.progress}%`;
  progressMessage.textContent = event.payload.message;
});

btnRestoreTest.addEventListener("click", async () => {
  const timestamp = backupSelect.value;
  if (!timestamp) {
    log(t("selectTestBackup"));
    return;
  }
  
  const targetPath = getFullTargetPath();
  if (!targetPath) {
    log(t("selectTargetFirst"));
    return;
  }
  
  log(`${t("verifyStarted")} ${timestamp}...`);
  
  try {
    const result = await invoke<{
      success: boolean;
      total_files: number;
      verified_files: number;
      failed_files: string[];
      message: string;
    }>("verify_backup", {
      targetPath: targetPath,
      timestamp: timestamp
    });
    
    if (result.success) {
      log(`✅ ${result.message}`);
    } else {
      log(`❌ ${result.message}`);
      for (const failure of result.failed_files) {
        log(`  - ${failure}`);
      }
    }
  } catch (e) {
    log(`${t("backupFailed")} ${e}`);
  }
});

showFilesBtn.addEventListener("click", async () => {
  const timestamp = backupSelect.value;
  if (!timestamp) {
    log(t("selectFilesBackup"));
    return;
  }
  
  const fullPath = getFullTargetPath();
  if (!fullPath) {
    log(t("selectTargetFirst"));
    return;
  }
  
  log(`${t("filesLoading")} ${timestamp}...`);
  
  try {
    interface BackupFileInfo {
      path: string;
      archive: string;
      archive_size_bytes: number;
      source_size_bytes: number;
    }
    
    interface BackupDetails {
      timestamp: string;
      items: BackupFileInfo[];
      total_source_size_bytes: number;
      total_archive_size_bytes: number;
      start_time: string;
      end_time: string;
      duration_seconds: number;
    }
    
    const details: BackupDetails = await invoke("list_backup_files", {
      targetPath: fullPath,
      timestamp: timestamp
    });
    
    const formatBytes = (bytes: number): string => {
      if (bytes === 0) return "0 B";
      const k = 1024;
      const sizes = ["B", "KB", "MB", "GB", "TB"];
      const i = Math.floor(Math.log(bytes) / Math.log(k));
      return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
    };
    
    log("");
    log(`=== ${t("filesHeader")} (${timestamp}) ===`);
    log("");
    log(`📅 ${t("filesBackupTime")}:`);
    log(`   ${t("filesStartTime")}: ${details.start_time}`);
    log(`   ${t("filesEndTime")}: ${details.end_time}`);
    log(`   ${t("filesDuration")}: ${details.duration_seconds} ${t("filesSeconds")}`);
    log("");
    log(`📊 ${details.items.length} ${t("filesItems")} | ${t("filesTotalOriginal")}: ${formatBytes(details.total_source_size_bytes)} | ${t("filesTotalArchive")}: ${formatBytes(details.total_archive_size_bytes)}`);
    log("");
    
    for (const item of details.items) {
      const origSize = formatBytes(item.source_size_bytes);
      const archSize = formatBytes(item.archive_size_bytes);
      const ratio = item.source_size_bytes > 0 
        ? ((1 - item.archive_size_bytes / item.source_size_bytes) * 100).toFixed(1)
        : "0";
      log(`📁 ${item.path}`);
      log(`   📦 ${item.archive} (${origSize} → ${archSize}, -${ratio}%)`);
    }
    
    log("");
    log("✅ " + t("filesHeader") + " " + t("completed"));
  } catch (error) {
    log("❌ " + t("error") + ": " + error);
  }
});

// Show manual apps handler
showManualAppsBtn.addEventListener("click", async () => {
  const timestamp = backupSelect.value;
  if (!timestamp) {
    log(t("selectBackupForManualApps"));
    return;
  }
  
  const fullPath = getFullTargetPath();
  if (!fullPath) {
    log(t("selectTargetFirst"));
    return;
  }
  
  try {
    const manualApps: string[] = await invoke("get_manual_apps_from_backup", {
      targetPath: fullPath,
      timestamp: timestamp
    });
    
    log("");
    log(`=== 📦 ${t("manualAppsTitle")} (${timestamp}) ===`);
    log("");
    
    if (manualApps.length === 0) {
      log(t("noManualApps"));
    } else {
      log(t("manualAppsDescription"));
      log("");
      
      // Sort alphabetically
      manualApps.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
      
      for (const app of manualApps) {
        log(`   • ${app}`);
      }
      
      log("");
      log(`📊 ${manualApps.length} ${manualApps.length === 1 ? "App" : "Apps"} gefunden`);
    }
    
    log("");
  } catch (error) {
    log(`❌ ${t("manualAppsError")} ${error}`);
  }
});

// Delete backup handler
btnDeleteBackup.addEventListener("click", async () => {
  const selectedBackup = backupSelect.value;
  if (!selectedBackup) {
    log(t("selectBackupFirst"));
    return;
  }
  
  const targetPath = getFullTargetPath();
  if (!targetPath) {
    log(t("selectTargetFirst"));
    return;
  }
  
  // Confirm deletion with native Tauri dialog
  const confirmed = await ask(t("confirmDeleteBackup") + "\n\n" + formatTimestamp(selectedBackup), {
    title: t("deleteBackup"),
    kind: "warning",
  });
  
  if (!confirmed) {
    return;
  }
  
  try {
    log(`Lösche Backup ${formatTimestamp(selectedBackup)}...`);
    await invoke("delete_backup", {
      targetPath: targetPath,
      timestamp: selectedBackup,
    });
    log(`✅ ${t("backupDeleted")}: ${formatTimestamp(selectedBackup)}`);
    await loadBackups();
  } catch (e) {
    log(`❌ ${t("deleteError")} ${e}`);
  }
});

copyLogBtn.addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(logOutput.textContent || "");
    log(t("copySuccess"));
  } catch (e) {
    log(`${t("copyError")} ${e}`);
  }
});

saveLogBtn.addEventListener("click", async () => {
  try {
    const path = await save({
      defaultPath: `backup-log-${new Date().toISOString().slice(0, 10)}.txt`,
      filters: [{ name: "Text", extensions: ["txt"] }],
    });
    
    if (path) {
      await writeTextFile(path, logOutput.textContent || "");
      log(`${t("saveSuccess")} ${path}`);
    }
  } catch (e) {
    log(`${t("saveError")} ${e}`);
  }
});

clearLogBtn.addEventListener("click", () => {
  logOutput.textContent = "";
  log(t("logCleared"));
});

// Settings dialog
btnSettings.addEventListener("click", () => {
  tempDefaultDirectories = [...config.default_directories];
  updateDefaultDirectoriesList();
  settingsDialog.showModal();
});

settingsCancelBtn.addEventListener("click", () => {
  settingsDialog.close();
});

settingsSaveBtn.addEventListener("click", async () => {
  config.default_directories = [...tempDefaultDirectories];
  await saveConfig();
  log(t("settingsSaved"));
  settingsDialog.close();
});

addDefaultDirectoryBtn.addEventListener("click", async () => {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: t("addFolder"),
    });
    
    if (selected) {
      const path = selected as string;
      const homeDir = await invoke<string>("get_home_dir");
      let displayPath = path;
      if (path.startsWith(homeDir)) {
        displayPath = "~" + path.substring(homeDir.length);
      }
      
      if (!tempDefaultDirectories.includes(displayPath)) {
        tempDefaultDirectories.push(displayPath);
        updateDefaultDirectoriesList();
      }
    }
  } catch (e) {
    log(`${t("selectError")} ${e}`);
  }
});

// User folder dialog
if (userFolderCloseBtn) {
  userFolderCloseBtn.addEventListener("click", () => {
    userFolderDialog.close();
  });
}

// Theme and language buttons
btnTheme.addEventListener("click", cycleTheme);
btnLanguage.addEventListener("click", toggleLanguage);

// Initialize
async function init(): Promise<void> {
  log(t("started"));
  await setupEventListeners();
  await loadConfig();
  await loadVolumes();
  await loadBackups();
  await checkFullDiskAccess();
  
  try {
    const hasHomebrew = await invoke<boolean>("check_homebrew");
    if (hasHomebrew) {
      log(t("homebrewFound"));
    } else {
      log(t("homebrewNotInstalled"));
    }
  } catch (e) {
    log(t("homebrewCheckFailed"));
  }
  
  try {
    const hasMas = await invoke<boolean>("check_mas");
    if (hasMas) {
      log(t("masFound"));
    } else {
      log(t("masNotInstalled"));
    }
  } catch (e) {
    // mas check failed silently
  }
}

init();

// Re-check FDA when window gains focus (user might have changed settings)
window.addEventListener("focus", () => {
  checkFullDiskAccess();
});


// Window state management via Rust backend
const appWindow = getCurrentWindow();

interface WindowState {
  width: number;
  height: number;
  x: number;
  y: number;
}

(async function initWindowState() {
  // Restore window state on startup (wait for window to be ready)
  await new Promise(resolve => setTimeout(resolve, 200));
  
  try {
    const state = await invoke<WindowState | null>("get_window_state");
    if (state && state.width && state.height) {
      if (state.width >= 960 && state.height >= 660) {
        await appWindow.setSize(new LogicalSize(state.width, state.height));
      }
      if (typeof state.x === 'number' && typeof state.y === 'number') {
        await appWindow.setPosition(new LogicalPosition(state.x, state.y));
      }
    }
  } catch (_e) {
    // Ignore errors during restore
  }
  
  // Save window state function
  const saveWindowState = async () => {
    try {
      const size = await appWindow.innerSize();
      const position = await appWindow.outerPosition();
      const scaleFactor = await appWindow.scaleFactor();
      
      const width = Math.round(size.width / scaleFactor);
      const height = Math.round(size.height / scaleFactor);
      
      await invoke("save_window_state", {
        width,
        height,
        x: position.x,
        y: position.y
      });
    } catch (_e) {
      // Ignore errors during save
    }
  };
  
  // Save on resize (debounced)
  let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
  appWindow.onResized(() => {
    if (resizeTimeout) clearTimeout(resizeTimeout);
    resizeTimeout = setTimeout(saveWindowState, 500);
  });
  
  // Save on move (debounced)
  let moveTimeout: ReturnType<typeof setTimeout> | null = null;
  appWindow.onMoved(() => {
    if (moveTimeout) clearTimeout(moveTimeout);
    moveTimeout = setTimeout(saveWindowState, 500);
  });
})();

// Global function for help menu
(window as unknown as { showHelp: () => void }).showHelp = async function() {
  try {
    await invoke("show_help_window");
  } catch (error) {
    console.error("Error showing help:", error);
  }
};
