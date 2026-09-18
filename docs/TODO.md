# ToDo – macOS Backup Suite (Stand 18.09.2026, main = 1.2.49)

## Offen

### 1. Adaptives Budget für die Metadaten-Probe (Vorschlag: 1.2.50)
- Heute: `MAX_METADATA = 512 MiB` fest (`src-tauri/src/backup/readback.rs`), ≈ 300 000 Einträge.
  Darüber greift `full_native_readback` (ganzer Baum schreiben/lesen/löschen).
- Ziel: Budget = min(¼ freier Platz im Temp-Ordner der internen SSD, 8 GiB), mindestens 512 MiB.
  Probe schreibt nur Header, keine Nutzdaten → Fallback praktisch nie mehr nötig.
- Prüfen: `space_preflight`, `LimitedMetadata`-Platzprüfung, `extraction_budget`; Test mit kleinem Budget bleibt.
- Release-Notes `releases/1.2.50.md`, README-Abschnitt „Rückleseprüfung“ ergänzen.

### 2. Durchsatzbegrenzung zur Laufzeit ändern (Regler im Fortschrittsdialog)
- Plan liegt vor (17.09.): `docs/plan-laufzeit-drossel.md`.
- Kern: `Bucket::set_rate`, `throttle::set_rate`, Tauri-Command, Slider in der UI, optional ins Profil übernehmen.
- Hinweis Nutzer: Problem ist die Temperatur des USB-Controllers, nicht der SSD – keine Sensorregelung möglich, manuelles Nachregeln.

### 3. Explizite Ausschlüsse pro Profil (Grundsatzentscheidung des Nutzers)
- z. B. `target/`, `node_modules/`, `.build/` bei Entwicklungsordnern.
- App verweigert bisher bewusst stille Ausschlüsse (`BACKUP_EXTRA_EXCLUDES` → Fehler). Wenn gewünscht:
  sichtbar in Einstellungen, im Protokoll und in `metadata.json`, damit ein Restore die Lücke kennt.
- Erst entscheiden, ob das überhaupt gewollt ist.

### 4. Ursache der Backup02-Auswürfe bestätigen (Diagnose)
- Kernel-Log mit sudo:
  `sudo log show --start "2026-09-18 08:25:00" --end "2026-09-18 08:27:30" --predicate 'process == "kernel"' | grep -iE "usb|disk|apfs|reset|terminat"`
- Bridge-Chip identifizieren: `system_profiler SPUSBDataType | grep -A12 -i nvme`
- TRIM-Status: `diskutil info /Volumes/Backup02 | grep -i trim`
- Ergebnis eines Backups mit 1.2.49 abwarten (Rückleseprüfung läuft jetzt über die interne SSD).

### 5. Einstellungsdialog: technische Abschnitte ans Ende (UI)
- Heute (`index.html` ab Z. 136): Standard-Ordner → Software-Inventar → Safari → Browser → Unbeaufsichtigtes Backup → **🌡️ Durchsatzbegrenzung** → App-Einstellungen.
- Ziel: inhaltliche Auswahl (Ordner, Inventar, Safari, Browser, App-Einstellungen) oben; technische Schutz-/Laufzeitoptionen
  (🌙 Unbeaufsichtigtes Backup, 🌡️ Durchsatzbegrenzung inkl. Cache-Option) als letzte Abschnitte vor den Dialog-Buttons.
- Nur `<div class="settings-section">`-Blöcke in `index.html` umsortieren; IDs/i18n-Keys bleiben, kein Backend betroffen.
- Ggf. gemeinsame Zwischenüberschrift „Technische Einstellungen“.

### 6. Echte Inkremente statt Verzeichnis-Granularität (Architekturthema)
- Heute: „Inkrementell“ = ein **ganzes Quellverzeichnis** wird nur dann per Hardlink übernommen, wenn sich **keine einzige Datei**
  darin geändert hat (`same_source_version` in `lib.rs` ~2416). Sonst wird das gesamte Archiv neu geschrieben.
- Beleg 15.09. → 18.09.: `~/Pictures` 165,6 GB komplett neu, obwohl nur 151 geänderte + 63 neue Dateien (0,37 GB) –
  Fotos-Mediathek (`Photos.sqlite`, Spotlight-Index) ändert sich bei jedem Öffnen. `reused_archive_size_bytes` = 22 MB von 317 GB.
- Bei Fotos-Mediathek, Parallels-VMs, Mail, Messages greift die Wiederverwendung praktisch nie → jedes Backup ≈ Vollbackup.
- Optionen (Aufwand steigend):
  a) Große, sich ständig ändernde Bundles als **eigene Quellwurzeln** anlegen (z. B. `Photos Library.photoslibrary/originals`
     getrennt von `database/`) – dann wird der große, stabile Teil per Hardlink übernommen. Kleine Änderung: Quellenliste/Profil.
  b) **Datei-Granulare Inkremente**: pro Quelle nur geänderte/neue Dateien in ein Delta-Archiv; Manifest verweist für
     unveränderte Dateien auf das Archiv des Vorgängers. Restore muss dann mehrere Archive kombinieren; Aufbewahrung darf
     referenzierte Vorgänger nicht löschen. Prüfsummen-Manifeste existieren bereits (`inventories/<ts>/manifests`).
  c) Chunk-Deduplizierung (à la borg/restic) – Neubau des Speicherformats, nicht empfohlen.
- Empfehlung: a) sofort möglich; b) als 1.3-Thema planen.

### 7. Kleinkram

- Separate Funktion „Backup prüfen“ liest alle Archive noch einmal – ggf. Hinweis in README, dass sie seltener nötig ist.

## Erledigt
- 1.2.46 Durchsatzbegrenzung (Token-Bucket, SIGSTOP/SIGCONT-Governor, Einstellungen) – PR #1
- 1.2.47 Laufwerks-Cache-Flush vermeiden (fsync statt F_FULLFSYNC, serialisiert) – PR #2
- 1.2.48 Gebremstes, serialisiertes Löschen temporärer Bäume – PR #3
- 1.2.49 Rückleseprüfung großer Bäume über interne SSD, gebremstes Einlesen je Datei – PR #4
- Test `compressed_file_roundtrip…` unabhängig von `ditto --hfsCompression` (macOS 27) – PR #5
- BurnISOtoUSB: PR gemergt, Version 1.4.17
