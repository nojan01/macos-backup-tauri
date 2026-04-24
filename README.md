# 💾 macOS Backup Suite

<p align="center">
  <img src="src-tauri/icons/icon.png" alt="macOS Backup Suite" width="128">
</p>

<p align="center">
  <strong>Umfassendes Backup- und Wiederherstellungswerkzeug für macOS</strong>
</p>

<p align="center">
  <a href="https://github.com/nojan01/macos-backup-tauri/releases/latest">
    <img src="https://img.shields.io/github/v/release/nojan01/macos-backup-tauri?style=flat-square" alt="Latest Release">
  </a>
  <a href="https://github.com/nojan01/macos-backup-tauri/blob/main/LICENSE">
    <img src="https://img.shields.io/github/license/nojan01/macos-backup-tauri?style=flat-square" alt="License">
  </a>
</p>

---

## ✨ Features

### 🛡️ Allgemein
- Dieses Programm ist kein Backup Programm im klassischen Sinn. Es installiert
  nicht das Betriebssystem MacOS nach einem Crash oder während einer geplanten
  Neuinstallation. Dieses Software ermöglicht es Ihre Software/ Apps, bestimmte
  Betriebssystemsettings und Ihre persönlichen Ordner auf eine effiziente
  Art wieder herzustellen. Dies ist besonders dann hilfreich wenn Ihr System
  fehlerhaft ist, die Performance nicht Ihren Vorstellungen entspricht oder Ihr 
  System mit allen möglichen nicht mehr benötigten Daten zugemüllt ist.
  Programme, welche manuell, also nicht über den Apple App Store oder über Homebrew,
  installiert wurden, müssen auch manuell wieder hergestellt werden. Eine Liste dieser
  Programme wird erstellt.

### 📦 Backup
- **Ordner-Backup** – Wichtige Verzeichnisse als komprimierte Archive (.tar.zst)
- **Homebrew** – Paketlisten (Brewfile) + optionaler Cache (max. 2 GB)
- **Mac App Store** – Alle installierten MAS-Apps
- **VS Code** – Erweiterungen und Einstellungen
- **Safari** – Lesezeichen, Leseliste, Erweiterungen, Preferences
- **Konfigurationsdateien** – SSH, Git, Shell-Configs

### ⚡ Parallele Verarbeitung (NEU in v1.1)
| Feature | Parallelität | Zeitersparnis |
|---------|-------------|---------------|
| MAS-Installation | 4 gleichzeitig | ~60-80% |
| VS Code Extensions | 6 gleichzeitig | ~50-70% |
| Backup-Verifizierung | 4 Threads | ~40% |

### 🔄 Quick-Restore Modus
Essentielle Tools in unter 10 Minuten:
- **Basis-Tools:** git, vim, python, node, wget, curl, jq, zsh
- **Essential Apps:** VS Code, iTerm2, Chrome, Firefox, Alfred, Raycast

### 🛡️ Sicherheit
- SHA-256 Hash-Verifizierung aller Archive
- Vollständige Backup-Metadaten in JSON
- Automatische Bereinigung unvollständiger Backups

### 🔗 Symlink-Handling

Das Backup behandelt symbolische Links bewusst **als Symlinks** und folgt ihnen
nicht — weder beim Archivieren noch beim Berechnen von Snapshots oder Größen.
Das hat konkrete Konsequenzen, die Sie kennen sollten:

- **Archivierung (tar):** `tar` speichert Symlinks standardmäßig als Links,
  nicht als Kopie des Zielinhalts. Ein Symlink, der auf ein Verzeichnis
  außerhalb des Backup-Scopes zeigt, wird also nur als Verweis gesichert.
  Zeigt der Link nach der Wiederherstellung ins Leere, ist das **kein Fehler
  des Backups**, sondern ein Hinweis, dass das Ziel selbst nicht Teil der
  gesicherten Ordnerliste war.
- **Inkrementelle Snapshots:** Der Manifest-Vergleich (`follow_links(false)`)
  erkennt **Änderungen am Symlink selbst** (Ziel-Pfad, mtime), aber **nicht**
  Änderungen am referenzierten Inhalt. Wenn nur das Ziel eines Symlinks
  modifiziert wird und das Ziel **außerhalb** des gesicherten Baumes liegt,
  erscheint das Archiv als unverändert und wird per Hardlink wiederverwendet.
- **Größenberechnung / Platzbedarf:** Symlinks zählen mit 0 Bytes. Der
  Pre-Flight-Check „freier Speicherplatz" folgt Symlinks nicht und
  überschätzt daher nichts; er kann aber _unterschätzen_, falls Sie einen
  Ordner per Symlink an anderer Stelle einbinden und den Zielordner
  **zusätzlich** zur Sicherungsliste hinzufügen (→ Inhalt wird doppelt
  archiviert).
- **Restore / Extraktion:** Beim Zurückspielen werden Symlinks 1:1
  rekonstruiert. Existiert das ursprüngliche Zielsystem nicht mehr (z. B.
  externe Homebrew-Pfade nach Hardware-Wechsel), bleibt der Link als
  „dangling symlink" bestehen, bis die referenzierten Pfade wiederhergestellt
  werden.
- **Sicherheit:** Die Archiv-Integritätsprüfung vor dem Extrahieren lehnt
  Einträge mit `..`-Komponenten und absoluten Pfaden ab. Symlinks mit
  absoluten Zielen werden unverändert geschrieben — prüfen Sie nach einem
  Restore über ein Fremdsystem, ob die Symlinks in Ihrem Home-Verzeichnis
  auf erwartete Pfade zeigen.

**Empfehlung:** Vermeiden Sie Symlinks, die aus dem Backup-Scope in
ungesicherte Bereiche zeigen, wenn diese Inhalte mit der Wiederherstellung
zurückkehren sollen. Fügen Sie stattdessen das Zielverzeichnis direkt in die
Sicherungsliste ein.

---

## 📥 Installation

### Download
Laden Sie die neueste Version herunter:
➡️ **[macOS Backup Suite v1.1.0](https://github.com/nojan01/macos-backup-tauri/releases/latest)**

### Voraussetzungen
- macOS 12.0 oder neuer
- [Homebrew](https://brew.sh) (empfohlen)
- Festplattenvollzugriff (Full Disk Access) für vollständige Backups

### Erste Schritte
1. DMG öffnen und App nach `/Applications` ziehen
2. Systemeinstellungen → Datenschutz → Festplattenvollzugriff → App hinzufügen
3. App starten und Backup-Ziel auswählen

---

## 🖥️ Screenshots

<p align="center">
  <img src="docs/screenshots/main-window.png" alt="macOS Backup Suite - Hauptfenster" width="700">
  <br>
  <em>Hauptfenster mit Backup-Übersicht, Ordnerauswahl und Protokoll</em>
</p>

---

## 🛠️ Entwicklung

### Technologie-Stack
- **Frontend:** TypeScript, HTML, CSS (Vanilla)
- **Backend:** Rust (Tauri 2.x)
- **Kompression:** zstd (mit gzip-Fallback)

### Build
```bash
# Dependencies installieren
npm install

# Development-Server starten
npm run tauri dev

# Production-Build erstellen
npm run tauri build

# DMG in App einbetten
./embed-dmg.sh
```

### Projektstruktur
```
macos-backup-tauri/
├── src/                    # TypeScript Frontend
│   ├── main.ts
│   └── styles.css
├── src-tauri/              # Rust Backend
│   └── src/lib.rs
├── public/
│   └── help.html           # Hilfe-Dokumentation
└── index.html
```

---

## 📋 Changelog

### v1.1.0 (Dezember 2025)
- ⚡ Parallele MAS-Installation (4×)
- ⚡ Parallele VS Code Extension Installation (6×)
- ⚡ Parallele Backup-Verifizierung (4 Threads)
- 🔄 Quick-Restore Modus für essenzielle Pakete
- 🧭 Safari-Einstellungen Backup (Lesezeichen, Erweiterungen, etc.)
- 🍺 Homebrew-Cache Backup (max. 2 GB, Offline-Installation)
- 📖 Aktualisierte Hilfe-Dokumentation

### v1.0.0 (Dezember 2025)
- Initiales Release
- Ordner-Backup mit zstd-Kompression
- Homebrew, MAS, VS Code Backup
- Vollständige Wiederherstellung
- SHA-256 Verifizierung

---

## 📄 Lizenz

MIT License – siehe [LICENSE](LICENSE)

---

<p align="center">
  Made with ❤️ for macOS
</p>
