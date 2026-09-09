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
- **Homebrew** – Paketlisten (Brewfile) + optionaler vollständiger Download-Cache
- **Mac App Store** – Alle installierten MAS-Apps
- **App-Einstellungen** – Eigene Checkboxen für VS Code (User-Einstellungen, Profile, Snippets und Erweiterungsliste), ChatGPT und Codex; standardmäßig aktiv, vorhandene Quellen werden vor jedem Backup neu erkannt.
- **Codex** – Konfiguration, zusätzliche Konfigurationsprofile, globale Regeln, eigene Skills und gemeinsame lokale App-Einstellungen. Kein vollständiger Chatverlauf, keine Arbeitskopien, keine Plugin-Binärdateien oder Codex-Anmeldedaten. Die erkannten Pfade sind im Einstellungsdialog einsehbar.
- **Optionale App-Quellen** – Fehlende optionale Pfade werden nicht hinzugefügt. Zugriffsfehler brechen die Prüfung ab. Bereits ausgewählte übergeordnete Ordner und gemeinsam verwendete Einstellungspfade werden beim Ergänzen berücksichtigt. Manuell ausgewählte Quellen bleiben unabhängig von den Checkboxen enthalten. Apps vor dem Backup schließen.
- **Wiederherstellung der App-Einstellungen** – Normale Archive mit vollständiger Inhalts- und Rückleseprüfung; im Wiederherstellungsdialog anhand ihrer ursprünglichen Pfade auswählbar.
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
- Fehlerhafte Archive werden nicht als erfolgreiche Sicherung übernommen; unterbrochene Backups können fortgesetzt werden

### Zuverlässigkeit der Datensicherung

- Ausgewählte Ordner werden vollständig archiviert, einschließlich leerer Verzeichnisse,
  versteckter Dateien, `Logs`, Caches und `node_modules`. Es gibt keine stillen Ausschlüsse.
  Die frühere Umgebungsvariable `BACKUP_EXTRA_EXCLUDES` führt bei gesetztem Wert zu einer
  Fehlermeldung, damit eine alte Konfiguration keine Daten unbemerkt auslässt.
- Verweigert macOS einen Dateizugriff bei gesperrtem Bildschirm, pausiert die
  Sicherung mit einer sichtbaren Entsperr-Meldung. Nach dem Entsperren wird derselbe
  Zugriff erneut versucht. Abbrechen bleibt wirksam; fehlt die Berechtigung auch
  danach, wird ein Fehler gemeldet. Die App ändert weder Sperre noch Dateirechte.
- Vor den vollständigen Inhaltsprüfungen werden alle ausgewählten Quellen rekursiv
  auf Zugriff geprüft (Dateistatus, ein Byte Leseprobe, Attribute und ACLs). Probleme
  werden gesammelt gemeldet, mit bis zu 100 konkreten Pfaden. Die Vorprüfung ist
  abbrechbar und ersetzt weder vollständiges Einlesen noch Rückleseprüfung.
- Wird eine Datei genau während des Einlesens ersetzt oder geändert, beginnt das
  Einlesen dieser Datei bis zu dreimal neu. Dauerhafte Änderungen und Zugriffsfehler
  bleiben Fehler; keine ausgewählten Dateidaten werden deswegen übersprungen.
- Inkrementelle Vergleiche lesen SHA-256-Prüfsummen der Dateiinhalte sowie Dateityp,
  nanosekundengenaue Zeitstempel, Rechte, Eigentümer, Dateiflags, ACLs, erweiterte
  Attribute und Symlink-Ziele. Auch leere Verzeichnisse werden erfasst.
  Alte Manifeste mit ausschließlich Größe und Sekundenzeitstempel werden nicht wiederverwendet.
- Dateiquellen werden aus einem neuen, schreibgeschützten APFS-Snapshot gesichert.
  Vorprüfung, Archivierung, Rückleseprüfung und Abschlussprüfung lesen denselben
  eingefrorenen Stand. Das Öffnen oder Bearbeiten der Originale während der Sicherung
  löst deshalb keinen Archivfehler aus; auch Nutzungsattribute bleiben auf dem Stand
  des Snapshots. Inhalts-, Attribut- und Flag-Prüfungen werden nicht abgeschwächt.
  Die Suite verwendet `tmutil localsnapshot` und hängt den bestätigten Snapshot privat
  mit `mount_apfs` nur lesbar ein. Beschreibbare Quellvolumes müssen APFS verwenden
  und von Time Machine für lokale Snapshots berücksichtigt werden. Nicht unterstützte
  Quellen und verschachtelte Volume-Einhängepunkte werden vor dem Archivieren gemeldet;
  es gibt keinen stillen Rückfall auf einen veränderlichen Live-Stand.
  Nach Abschluss oder Abbruch wird die private Ansicht ausgehängt. Den freigebbaren
  lokalen Snapshot verwaltet Time Machine; bestehende Snapshots werden nicht gelöscht.
  `source-snapshots.json` dokumentiert den Dateistand und die ursprünglichen Quellpfade.
- Einzeldateien und Ordner werden mit macOS-System-tar im PAX-Format archiviert.
  Jedes neue oder wiederverwendete Datenarchiv wird probeweise in ein privates lokales
  Verzeichnis entpackt. Inhalte, Rechte, ACLs, erweiterte Attribute, Zeitstempel und
  Verknüpfungen werden mit der Quelle verglichen. Die Rückleseprüfung benötigt lokalen
  temporären Speicher für jeweils einen entpackten Quellordner; der Platz wird geprüft.
- Dateiflags werden zusätzlich als versionierte Metadaten im Archiv gespeichert,
  da System-tar unter anderem `UF_TRACKED` nicht serialisiert. Die SHA-256-Prüfsumme
  des Archivs umfasst diese Metadaten. Normale Wiederherstellung, Test-Restore und
  Rückleseprüfung setzen die Flags aus dem Archiv; das Original wird nicht benötigt.
  Die Rückleseprüfung verlangt weiterhin identische Flags. Kernelverwaltete Zustände
  wie Dateisystemkompression werden nicht durch bloßes Setzen eines Bits vorgetäuscht.
  Unveränderlichkeits- und Append-Schutz der privaten Zwischenkopie werden für das
  Verschieben gelöst und am endgültigen Restore-Ziel wieder gesetzt.
  Ältere Archive bleiben lesbar. Beim manuellen Entpacken nur mit System-tar bleiben
  dessen Einschränkungen bestehen; für die zusätzlichen Flags ist mindestens Suite v1.2.17 nötig.
- Das von macOS beim Kopieren neu vergebene Herkunftsattribut `com.apple.provenance`
  wird weiter archiviert und bei Quelländerungen berücksichtigt. Ausschließlich beim
  Vergleich der entpackten Kopie darf es abweichen. Dateiinhalte, ACLs, Resource Forks
  und alle übrigen erweiterten Attribute werden unverändert geprüft. Auch den Wert
  von `com.apple.quarantine` vergibt macOS beim Entpacken neu (mit anderem Zeitstempel
  und Herkunftsprogramm); er bleibt im Archiv erhalten. Beim Rücklesen muss das
  Quarantäneattribut weiterhin vorhanden sein, sein neu vergebener Wert darf abweichen.
  Der Quellvergleich berücksichtigt weiterhin den vollständigen ursprünglichen Wert.
  Fehlermeldungen
  nennen den betroffenen Pfad und das abweichende Merkmal, auch beim obersten Ordner.
- Archivieren, Strukturprüfung, Entpacken einschließlich macOS-Dateiattributen,
  Rücklesevergleich, Prüfsummen und Aufräumen haben eigene Statusmeldungen. Eine
  sekündliche Laufzeitanzeige bleibt auch bei stillen Unterprozessen aktiv; verstrichene
  Zeit wird nicht als gemessener Datei- oder Prozentfortschritt ausgegeben.
- Die Archivierung nutzt das bereits vollständig gelesene Manifest des APFS-Snapshots. Redundante
  Quellscans und der doppelte Archivdurchlauf vor derselben Rücklese-Extraktion entfallen.
  Der vollständige Vergleich mit dem entpackten Archiv, die frische Prüfung des Snapshots danach
  und die globale Abschlussprüfung bleiben erhalten.
- Fehlende oder unlesbare Quellen, Lese-/Schreibfehler und nicht unterstützte
  Spezialdateien wie FIFOs/Gerätedateien brechen die Sicherung ab. Echte Unix-Sockets
  innerhalb eines Quellordners werden als flüchtige Kommunikationsendpunkte übersprungen;
  ihre Pfade stehen im Protokoll und in `skipped-runtime-sockets.json` beim Backup.
  Die Ausnahme richtet sich nach dem Dateityp, nicht nach Namen oder Ordnern.
  Normale Dateien, auch mit `.sock` im Namen, bleiben vollständig enthalten. Fehlende ausgewählte
  Homebrew-/App-Store-Inventare sind ebenfalls Fehler. Die Auswahl lässt sich in den
  Einstellungen ändern. Ein Backup-Ziel innerhalb einer Quelle wird abgelehnt.
- Vor dem ersten Inhalts-Scan werden sämtliche ausgewählten Quellpfade auf Existenz,
  grundlegende Lesbarkeit und Überschneidung mit dem Ziel geprüft. Fehlende Einträge
  werden gesammelt gemeldet, bevor große Ordner gelesen werden. Gespeicherte Auswahlpfade
  werden dabei nicht automatisch entfernt oder stillschweigend übersprungen.
- Quellen werden vor dem Archivieren und nochmals vor dem Abschluss geprüft.
  Archive, Manifeste, Zwischenstände und Abschlussmetadaten werden über temporäre
  Dateien, Synchronisierung und atomisches Umbenennen veröffentlicht.
  `metadata.json` entsteht erst nach erfolgreicher Abschlussprüfung.
- Ein Wiederanlauf erstellt die ausgewählten Quellen erneut aus ihrem aktuellen
  Zustand. Bereits abgeschlossene Teilarchive werden nicht blind übernommen.
  Verifizierte, unveränderte Archive eines früheren **vollständigen** Backups können
  weiterhin per Hardlink wiederverwendet werden, ohne sie beim nächsten Lauf zu überschreiben.

**Betriebsgrenze:** Dies ist eine Dateisicherung ohne APFS-Volume-Snapshot und ohne
anwendungsspezifisches Datenbank-Backup. Datenbanken, virtuelle Maschinen und andere
schreibende Programme vor der Sicherung schließen. Erkannte Änderungen während der
Sicherung führen zu einem Fehler; eine atomare Momentaufnahme aller laufenden Apps
kann das Verfahren nicht garantieren. Erforderlicher Festplattenvollzugriff muss erteilt
sein. Auch ein erfolgreicher Test ersetzt keine regelmäßig geprüfte zweite Sicherung
auf einem unabhängigen Datenträger.

Beim Restore werden Dateien dem wiederherstellenden Benutzer zugeordnet; frühere
Eigentümer-IDs werden nicht privilegiert übernommen. Bei „Überschreiben“ werden auch
Metadaten vorhandener Verzeichnisse wiederhergestellt. Ohne „Überschreiben“ bleiben
vorhandene Einträge und ihre Metadaten erhalten.

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
- **Größenberechnung / Platzbedarf:** Symlinks enthalten nur den Verweis. Ihr
  Zielinhalt wird nur gesichert, wenn er selbst innerhalb der ausgewählten Quellen
  liegt. Zusätzlich zum Archiv benötigt die Rückleseprüfung lokalen temporären Platz.
- **Restore / Extraktion:** Beim Zurückspielen werden Symlinks 1:1
  rekonstruiert. Existiert das ursprüngliche Zielsystem nicht mehr (z. B.
  externe Homebrew-Pfade nach Hardware-Wechsel), bleibt der Link als
  „dangling symlink" bestehen, bis die referenzierten Pfade wiederhergestellt
  werden.
- **Sicherheit:** Vor dem Restore werden die SHA-256-Prüfsummen aller ausgewählten Archive und deren tar-Header geprüft. Absolute Eintragspfade, `..`, Pfadkollisionen, Gerätedateien und Einträge unterhalb eines Archiv-Symlinks werden abgelehnt. Die Extraktion erfolgt zuerst in einem privaten Zwischenverzeichnis. Symlinks werden als Links wiederhergestellt; beim Zusammenführen werden vorhandene Ziel-Symlinks nicht als Verzeichnisse verfolgt.

**Empfehlung:** Vermeiden Sie Symlinks, die aus dem Backup-Scope in
ungesicherte Bereiche zeigen, wenn diese Inhalte mit der Wiederherstellung
zurückkehren sollen. Fügen Sie stattdessen das Zielverzeichnis direkt in die
Sicherungsliste ein.

---

## 📥 Installation

### Download
Laden Sie die neueste Version herunter:
➡️ **[macOS Backup Suite v1.2.22](https://github.com/nojan01/macos-backup-tauri/releases/latest)**

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


## Software-Inventare ab 1.2.21

Homebrew-Bundle-Einträge für Cargo, npm, Go, uv und krew sowie die plattformspezifischen Flatpak-/WinGet-Einträge werden als solche erkannt. Zusätzliche Pakete werden beim Restore über einen aus validierten Literalen neu erzeugten Brewfile installiert; Ruby-Code aus dem gesicherten Inventar wird nicht ausgeführt. Quell-URLs von Cargo/uv bleiben erhalten. Die Ausgabe von `mas list` wird neben älteren MAS-Brewfile-Einträgen unterstützt. Alle ausgewählten Software-Inventare werden vor Snapshot, Datei-Scan und Archivierung validiert.

## Fortschrittsanzeige ab 1.2.20

Während einer Vorbereitung ohne bekannte Gesamtmenge bleibt der Balken als Aktivitätsanzeige sichtbar. Sobald Prozentwerte vorliegen, werden sie als Zahl und Balken angezeigt. Phasenwechsel setzen den Fortschritt nicht zurück; bei Ende oder Abbruch stoppt die Animation.

## Archivformat ab 1.2.19

Metadaten werden in PAX-Headern gespeichert. Echte Dateien und Verzeichnisse mit `._` im Namen bleiben eigenständige Einträge; sie werden nicht als AppleDouble-Metadaten verbraucht. Erweiterte Attribute einschließlich Resource Forks, ACLs und Dateiflags werden weiterhin gesichert und beim Rücklesen geprüft. Ein Formatmerkmal im internen Metadateneintrag steuert das Entpacken; vorhandene Archive ohne dieses Merkmal verwenden weiterhin die bisherige AppleDouble-Wiederherstellung.

Beim Fortsetzen werden vorhandene, bereits rückgelesene Archive nur übernommen, wenn das vollständige Quellmanifest zum neuen APFS-Snapshot passt und die Archiv-Prüfsumme stimmt. Veränderte Quellen und beschädigte Archive werden neu erstellt. Erfolgreiche Zwischenstände bleiben bei einem weiteren Abbruch erhalten.

## Restore-Verhalten und Tests

- Ohne **Überschreiben** werden vorhandene Ordner zusammengeführt: fehlende Dateien kommen hinzu, vorhandene Dateien und Links bleiben erhalten. Mit Überschreiben werden einzelne Dateien/Links atomar ersetzt. Konflikte zwischen einer Datei und einem Verzeichnis werden als Fehler gemeldet; Verzeichnisbäume werden nicht automatisch gelöscht.
- Alle ausgewählten Archive müssen vor Beginn die Hash- und Inhaltsprüfung bestehen. Ein Test-Restore führt dieselbe Vorprüfung aus und schreibt anschließend in einen eigenen Unterordner.
- Neue Archive erhalten einen Namen mit einem Hash des vollständigen Quellpfads. Bestehende gzip-/zstd-Backups sind weiterhin lesbar. Alte Backups mit kollidierenden Archivnamen oder ungültigen Hashes werden abgelehnt; bereits überschriebene Archivdaten lassen sich dadurch nicht zurückholen. Dafür ist ein neues Backup erforderlich.
- Archive werden zunächst separat erstellt und danach umbenannt. Auch beim Fortsetzen einer Sicherung bleiben bereits vorhandene, eventuell mit älteren Backups hartverlinkte Archive bei Fehlern unberührt.
- Das Backup-Menü unterscheidet **Metadaten lesbar** von **verifiziert**. Ein Häkchen erscheint erst nach einer erfolgreichen Prüfung in der aktuellen Sitzung; beim Neuladen wird es zurückgesetzt. Restore prüft die Daten unabhängig davon erneut.
- Safari enthält auch die allgemeinen Preferences und den Favicon-Cache. Safari sollte vor einem echten Restore beendet sein, damit die laufende Anwendung die zurückgespielten Daten nicht wieder überschreibt.
- Homebrew-Paketnamen werden aus dem Brewfile gelesen und über direkte Prozessargumente installiert. Ruby-/Shell-Code aus der Datei wird nicht ausgeführt. Bundle-spezifische Optionen für Dienste und Verlinkungen werden nicht automatisch angewendet; dies wird im Protokoll angezeigt. MAS- und VS-Code-Installationsfehler führen auch bei Teilerfolg zu einer Fehlermeldung.
- Neue Standardordner verwenden `~/Documents` und `~/Desktop`. Bereits gespeicherte absolute Pfade bleiben absolute Ziele, auch bei einem anderen Benutzerkonto. Für einen solchen Umzug zuerst den Test-Restore verwenden und die Dateien in das gewünschte Konto übernehmen.
- Die Zwischenablage für große Verzeichnis-Restores liegt auf dem Ziellaufwerk. Dort wird freier Platz für das entpackte Archiv benötigt. Ein Restore über mehrere Elemente ist keine gemeinsame Transaktion; bei einem späteren Fehler können vorherige Elemente bereits wiederhergestellt sein.

Regressionstests (ausschließlich temporäre Testdaten; Paketinstallationen werden durch Testprogramme ersetzt):

```sh
npm test
npm run test:rust
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

Die Rust-Tests prüfen unter anderem Archivkollisionen, SHA-256, korrupte Archive, Datei-Konflikte, Safari/Cache, Symlinks, alte Kompressionsformate und Installationsfehler. Die UI-Tests prüfen die sichere Verarbeitung von Dateinamen und die Ergebnisanzeige. Ein vollständiger macOS-Restore einschließlich Full Disk Access und echter App-Store-/Homebrew-Installationen muss zusätzlich in einem separaten Testkonto oder einer VM geprüft werden.

## Sprache und platzsparende Rückleseprüfung (1.2.22)

Einstellungen, Dialoge, Hilfetexte, Fußzeile und laufende Prüfmeldungen folgen der ausgewählten Sprache. Beim Sprachwechsel bleiben der laufende Status und der Prozentwert erhalten; auch das Protokoll wird mit seinen ursprünglichen Zeitpunkten neu dargestellt. Dateipfade und externe Werkzeugausgaben werden nicht übersetzt.

Die automatische Archivprüfung entpackt gewöhnliche Dateiinhalte nicht mehr vollständig auf die interne SSD. Alle Bytes werden aus dem Archiv gelesen und mit SHA-256 gegen das Quellmanifest geprüft. Kleine Metadatenproben prüfen weiterhin die native Wiederherstellung von ACLs, xattrs, Dateiflags, Zeitstempeln sowie symbolischen und harten Links. Für das macOS-Kompressionsflag wird eine kleine komprimierbare Probe verwendet; der Inhaltsvergleich verwendet weiterhin den vollständigen Original-Datenstrom. Ältere AppleDouble-Metadaten bleiben lesbar.

Der temporäre Metadatenstrom ist auf 512 MiB pro Archiv begrenzt; vor dem Entpacken werden zusätzlicher Platz für Dateisystemeinträge und eine Reserve von 2 GiB geprüft. Große gewöhnliche Dateien und virtuelle Festplatten benötigen daher keine zweite vollständige temporäre Kopie. Außergewöhnlich große Metadaten führen zu einer ausdrücklichen Platz-/Limitmeldung, nicht zum stillen Weglassen von Attributen. Ein vom Benutzer gestarteter **Test-Restore** schreibt weiterhin das gewählte Element vollständig in dessen Test-Zielordner.

Sparse-Dateien werden als normale logische PAX-Daten gespeichert (Nullbereiche werden komprimiert) und beim Wiederherstellen wieder platzsparend mit Sparse-Dateien geschrieben.
