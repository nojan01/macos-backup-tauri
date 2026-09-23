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
- **Ordner-Backup** – Wichtige Verzeichnisse als Container aus AppleArchive/LZFSE-Teilarchiven (.aarset)
- **Homebrew** – Paketlisten (Brewfile) + optionaler vollständiger Download-Cache
- **Mac App Store** – Alle installierten MAS-Apps
- **App-Einstellungen** – Eigene Checkboxen für VS Code (User-Einstellungen, Profile, Snippets und Erweiterungsliste), ChatGPT und Codex; standardmäßig aktiv, vorhandene Quellen werden vor jedem Backup neu erkannt.
- **Codex** – Konfiguration, zusätzliche Konfigurationsprofile, globale Regeln, eigene Skills und gemeinsame lokale App-Einstellungen. Kein vollständiger Chatverlauf, keine Arbeitskopien, keine Plugin-Binärdateien oder Codex-Anmeldedaten. Die erkannten Pfade sind im Einstellungsdialog einsehbar.
- **Optionale App-Quellen** – Fehlende optionale Pfade werden nicht hinzugefügt. Zugriffsfehler brechen die Prüfung ab. Bereits ausgewählte übergeordnete Ordner und gemeinsam verwendete Einstellungspfade werden beim Ergänzen berücksichtigt. Manuell ausgewählte Quellen bleiben unabhängig von den Checkboxen enthalten. Apps vor dem Backup schließen.
- **Wiederherstellung der App-Einstellungen** – Normale Archive mit vollständiger Inhalts- und Rückleseprüfung; im Wiederherstellungsdialog anhand ihrer ursprünglichen Pfade auswählbar.
- **Safari** – Lesezeichen, Leseliste, Erweiterungen, Preferences
- **Konfigurationsdateien** – SSH, Git, Shell-Configs
- **Durchsatzbegrenzung** – Optionales Limit in MB/s für Lese- und Schreibzugriffe auf das Backup-Ziel (Backup, Prüfung, Wiederherstellung), z. B. für externe SSDs, deren USB-Controller bei vollem Tempo überhitzt. Siehe [Abschnitt unten](#durchsatzbegrenzung-ab-1246).
- **Eingehängte Netzwerkziele** – NFS-, SMB- und DualBeam/rclone-Mounts werden auch außerhalb von `/Volumes` als Backup-Ziel angeboten. Ein Ziel-Unterordner kann gewählt werden, selbst wenn der Mount-Wurzelordner nicht beschreibbar ist. Vor dem Backup werden Schreiben, Umbenennen und Rücklesen im Zielordner geprüft; während des Backups wird die Mount-Identität kontrolliert. Die Quelle muss weiterhin auf einem snapshotfähigen APFS-Volume liegen. Bei rclone mit VFS-Schreibcache bestätigt die unmittelbare Rückleseprüfung nur die Mount-Ansicht; der spätere Upload zum Cloud-Anbieter muss separat abgeschlossen sein. Der gemeldete freie Speicher eines rclone-Mounts kann ein Schätzwert sein.

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
  Bei einem nativen AppleArchive-Fehler werden dessen prozessbezogene Fehlereinträge
  aus dem macOS-Systemprotokoll ergänzt. So bleibt ein verstecktes „Operation not
  permitted“ samt betroffenem Pfad erkennbar, auch wenn `aa` auf stderr nur
  „Archive encoding failed“ ausgibt. Ist die Systemdiagnose nicht verfügbar, bleibt
  der ursprüngliche Fehler bestehen; die App überspringt keine Dateien.
- **Aufwecken ist nicht Entsperren:** Ein ausgeschalteter Monitor verhindert das
  Backup nicht, solange der Mac wach ist und die Dateien zugänglich sind. Im
  Systemruhezustand läuft die normale Backup-Verarbeitung nicht weiter. Ein
  Weckprogramm kann den Mac aufwecken, entsperrt aber nicht die Benutzersitzung.
  Geschützte Dateien können deshalb weiterhin unzugänglich bleiben. Auch ein
  automatisch aufgeweckter Mac kann beim Backup bis zum manuellen Entsperren warten;
  ein unbeaufsichtigter Nachtlauf ist dadurch nicht garantiert. Auch Time Machine
  kann davon betroffen sein, siehe [Apple Support](https://support.apple.com/de-de/102220).
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
- Einzeldateien und Ordner werden aus dem unveränderten nativen AppleArchive-Datenstrom
  in Abschnitte mit höchstens **1 GiB unkomprimierten Daten** aufgeteilt. Grenzen dürfen
  mitten durch eine große Einzeldatei oder einen Resource Fork laufen. Jeder Abschnitt
  ist separat mit Apples `/usr/bin/aa` und LZFSE komprimiert. Alle Teile liegen mit
  Versionsindex und SHA-256-Prüfsummen in einer selbstständigen `.aarset`-Datei.
  Dieses Containerformat benötigt die Suite zur Wiederherstellung; es ist selbst kein
  direkt mit `aa` entpackbares Einzelarchiv. Native `.aar`-Backups der Alpha 1 bleiben
  lesbar; TAR-Backups werden in diesem Entwicklungszweig nicht unterstützt.
- Jedes Teil wird vom Ziel zurückgelesen, anhand seiner komprimierten Prüfsumme geprüft,
  entpackt und erneut mit den ursprünglichen Abschnittsbytes verglichen. Danach werden
  ausschließlich seine privaten temporären Daten entfernt. Die gesicherten Teile bleiben
  erhalten. Parallel prüft der native Datenstromvergleich Dateiinhalt, Rechte, ACLs,
  erweiterte Attribute einschließlich vollständiger Resource Forks, Dateiflags,
  Nanosekunden-Zeitstempel und Hard-/Symlinks gegen das eingefrorene Quellmanifest.
  Die Wiederherstellung setzt die Abschnitte in ihrer geprüften Reihenfolge zusammen
  und übergibt den nativen Datenstrom an AppleArchive. UID/GID werden dem ausführenden
  Benutzer zugeordnet. Ein Test-Restore prüft zusätzlich die tatsächliche Wiederherstellung.
- Bei genügend freiem RAM erstellt und prüft die Suite **128-MiB-Teile im Arbeitsspeicher**.
  Rohdaten, komprimiertes Teil und Rücklesekopie bleiben dort und werden nach jedem
  Teil freigegeben. Der Speicherzustand wird für jeden Abschnitt erneut geprüft.
  Sinkt der freie RAM, werden der aktuelle und folgende Teile auf der internen SSD
  verarbeitet; die vorhandene 5-GiB-Platzprüfung erfolgt dann vor der Nutzung.
  Ein RAM-Modus ist keine Garantie gegen macOS-Swap bei anderweitigem Speicherdruck.
  Für große bestehende 1-GiB-Teilarchive bleibt der SSD-Lesepfad erhalten.
  Es wird keine vollständige Prüfkopie des Quellordners angelegt. Auf dem Ziel
  bleiben Archivbedarf, Eintragskosten, 10 % Aufschlag und 8 GiB Reserve
  konservativ berücksichtigt. Große Quellen werden zuerst verarbeitet.
- Ein unvollständiger Container wird nicht veröffentlicht. Fehlende, beschädigte,
  vertauschte oder zu große Teile sowie ein ungültiger Index führen zum Abbruch.
  Unveränderte `.aarset`-Container lassen sich weiterhin als Ganzes per Hardlink
  wiederverwenden. Die Durchsatzbegrenzung gilt für das Schreiben und Rücklesen der
  Teile auf dem geschützten Zielvolume. Prüfsummen ersetzen keine physische
  Datenträgerprüfung; Betriebssystem- und Laufwerks-Caches gelten weiterhin.
- Dateiflags sind native AppleArchive-Metadaten. Kernelverwaltete Zustände wie
  Dateisystemkompression werden nicht durch bloßes Setzen eines Bits vorgetäuscht.
  Bereits komprimierte Quelldateien werden bei der nativen Wiederherstellung ausdrücklich
  mit LZFSE behandelt. Ihre Dateiflags werden vor dem Zusammenführen kontrolliert;
  ein verlorenes Kompressionsmerkmal gilt als Fehler.
  Schutzflags privater Zwischenkopien werden nur für das Zusammenführen bzw. Aufräumen
  gelöst; Quellrechte und Quellattribute werden nicht verändert.
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
  Quellscans entfallen. Vor der Extraktion wird der vollständige AppleArchive-Index geprüft.
  Der vollständige Vergleich des abschnittsweise geprüften Datenstroms, die frische Prüfung des Snapshots danach
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

- **Archivierung:** AppleArchive speichert Symlinks standardmäßig als Links,
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
- **Sicherheit:** Vor dem Restore werden die SHA-256-Prüfsummen aller ausgewählten Archive und deren AppleArchive-Einträge geprüft. Absolute Eintragspfade, `..`, Pfadkollisionen, Gerätedateien und Einträge unterhalb eines Archiv-Symlinks werden abgelehnt. Die Extraktion erfolgt zuerst in einem privaten Zwischenverzeichnis. Symlinks werden als Links wiederhergestellt; beim Zusammenführen werden vorhandene Ziel-Symlinks nicht als Verzeichnisse verfolgt.

**Empfehlung:** Vermeiden Sie Symlinks, die aus dem Backup-Scope in
ungesicherte Bereiche zeigen, wenn diese Inhalte mit der Wiederherstellung
zurückkehren sollen. Fügen Sie stattdessen das Zielverzeichnis direkt in die
Sicherungsliste ein.

---

## 📥 Installation

### Download
Laden Sie die neueste Version herunter:
➡️ **[Aktuelle macOS Backup Suite](https://github.com/nojan01/macos-backup-tauri/releases/latest)**

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
- **Archivierung und Kompression:** AppleArchive mit LZFSE über macOS `/usr/bin/aa`

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

### Signierte App-Updates

Die App prüft beim Start still auf Updates; über den Button ⬇️ kann die Prüfung
auch manuell ausgelöst werden. Ein Update wird nur installiert, wenn das
Updater-Archiv mit dem projektspezifischen Tauri-Schlüssel signiert ist.

Bei einem Release muss zusätzlich zur DMG das von Tauri erzeugte Archiv samt
`.sig` sowie `latest.json` hochgeladen werden:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$HOME/.tauri/macos-backup-suite-updater.key"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
npm run tauri build
npm run make-updater-manifest -- <version> darwin-aarch64 "src-tauri/target/release/bundle/macos/macOS Backup Suite.app.tar.gz"
```

`latest.json`, `macOS Backup Suite.app.tar.gz` und dessen `.sig` gehören als
Assets in dasselbe GitHub-Release wie die DMG. Der private Schlüssel bleibt
lokal und wird niemals veröffentlicht.

Wird das Updater-Archiv nach dem Stapeln der Notarisierung von Hand neu
erzeugt, dürfen keine AppleDouble-Einträge (`._…`) hineingeraten – der Updater
bricht sonst mit „failed to unpack `._macOS Backup Suite.app`“ ab:

```bash
COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -czf "macOS Backup Suite.app.tar.gz" "macOS Backup Suite.app"
npx tauri signer sign --private-key-path "$TAURI_SIGNING_PRIVATE_KEY" --password "" "macOS Backup Suite.app.tar.gz"
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

## Archivformat ab 1.3.0-alpha.1

Metadaten werden nativ in AppleArchive gespeichert. Echte Dateien und Verzeichnisse mit `._` im Namen bleiben eigenständige Einträge; sie werden nicht als AppleDouble-Metadaten verbraucht. Erweiterte Attribute einschließlich Resource Forks, ACLs und Dateiflags werden weiterhin gesichert und beim Rücklesen geprüft. Es gibt keine zusätzlichen TAR-Metadateneinträge und keinen alten AppleDouble-Wiederherstellungspfad.

Beim Fortsetzen werden vorhandene, bereits rückgelesene Archive nur übernommen, wenn das vollständige Quellmanifest zum neuen APFS-Snapshot passt und die Archiv-Prüfsumme stimmt. Veränderte Quellen und beschädigte Archive werden neu erstellt. Erfolgreiche Zwischenstände bleiben bei einem weiteren Abbruch erhalten.

## Restore-Verhalten und Tests

- Ohne **Überschreiben** werden vorhandene Ordner zusammengeführt: fehlende Dateien kommen hinzu, vorhandene Dateien und Links bleiben erhalten. Mit Überschreiben werden einzelne Dateien/Links atomar ersetzt. Konflikte zwischen einer Datei und einem Verzeichnis werden als Fehler gemeldet; Verzeichnisbäume werden nicht automatisch gelöscht.
- Alle ausgewählten Archive müssen vor Beginn die Hash- und Inhaltsprüfung bestehen. Ein Test-Restore führt dieselbe Vorprüfung aus und schreibt anschließend in einen eigenen Unterordner.
- Neue Archive erhalten einen Namen mit einem Hash des vollständigen Quellpfads. Dieser Entwicklungszweig akzeptiert ausschließlich AppleArchive mit LZFSE. Alte Backups mit kollidierenden Archivnamen oder ungültigen Hashes werden abgelehnt; bereits überschriebene Archivdaten lassen sich dadurch nicht zurückholen. Dafür ist ein neues Backup erforderlich.
- Archive werden zunächst separat erstellt und danach umbenannt. Auch beim Fortsetzen einer Sicherung bleiben bereits vorhandene, eventuell mit älteren Backups hartverlinkte Archive bei Fehlern unberührt.
- Das Backup-Menü unterscheidet **Metadaten lesbar** von **verifiziert**. Ein Häkchen erscheint nach erfolgreicher Prüfung; der gespeicherte Prüfstatus wird beim Neuladen auf Gültigkeit geprüft. Restore prüft die Daten unabhängig davon erneut.
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

Die Rust-Tests prüfen unter anderem Archivkollisionen, SHA-256, korrupte Archive, Datei-Konflikte, Safari/Cache, Symlinks, ungültige Archivstrukturen und Installationsfehler. Die UI-Tests prüfen die sichere Verarbeitung von Dateinamen und die Ergebnisanzeige. Ein vollständiger macOS-Restore einschließlich Full Disk Access und echter App-Store-/Homebrew-Installationen muss zusätzlich in einem separaten Testkonto oder einer VM geprüft werden.

## Offene Punkte und Pläne

Offene Aufgaben stehen in [`docs/TODO.md`](docs/TODO.md); ausgearbeitete Pläne (z. B. Durchsatzlimit zur Laufzeit ändern) daneben in `docs/`.

## AppleArchive/LZFSE-Entwicklungszweig

Der TAR/Zstandard-Stand ist mit `frozen-tar-1.2.50` am Commit `7cf91b6`
eingefroren. Die Umstellung läuft auf `codex/applearchive-lzfse` als Version
`1.3.0-alpha.2`. Dies ist ein neues Backupformat ohne TAR-Kompatibilität.
Bestehende Dateien auf Backup-Laufwerken werden durch die Migration nicht gelöscht.

Die Archivierung schreibt direkt mit `/usr/bin/aa`; eine TAR-Kompressor-Pipeline
entfällt. Native Rückleseprüfung, SHA-256, APFS-Quellsnapshots und Abbruchschutz bleiben
aktiv. Nullbereiche werden komprimiert und mit `-enable-holes` wiederhergestellt.
Eine vollständige Rückleseprüfung erzeugt zusätzliche Lese-/Schreibarbeit.

Die Durchsatzbegrenzung steuert den Archivschreiber anhand des Dateiwachstums.
Archivlesevorgänge werden über einen gebremsten Eingabestrom geführt. Auch das
Einlesen und Aufräumen von Testkopien auf dem geschützten Ziel wird begrenzt.
Kurzzeitige Spitzen und macOS-Dateisystem-Metadaten sind dadurch nicht vollständig
begrenzt. Die optionale Vermeidung von `F_FULLFSYNC` bleibt verfügbar.

Die Ursachen der bisherigen mehrstündigen Abbrüche sind nicht abschließend geklärt.
Die Formatumstellung allein belegt keine Fehlerbehebung. Vor produktiver Freigabe
sind lange vollständige Backups und anschließende Test-Restores erforderlich.
