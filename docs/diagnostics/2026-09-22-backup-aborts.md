# Untersuchung der Backup-Abbrüche vom 21./22. September 2026

## Gesicherte Beobachtungen

- Die Abbrüche traten laut Nutzer bereits vor Einführung der Durchsatzbegrenzung
  mit drei verschiedenen Zieldatenträgern auf. Die Parallels-VM lief nicht.
- Zwei Screenshots zeigen einen Archivierungsfehler bei `~/Library/Preferences`:
  `tar: (null)` und anschließend `zstd: error 70 ... Broken pipe`.
- Ein anderer Lauf meldet einen E/A-Lesefehler beim Inhalt einer `.hds`-Datei
  innerhalb des schreibgeschützt eingehängten Quell-Snapshots. Das ist kein
  unmittelbarer Nachweis eines Fehlers auf dem Backup-Ziellaufwerk.
- Das Systemprotokoll vom 22.09. um 04:08:08 zeigt, dass die Backup-App ihren
  eigenen Snapshot über `umount` aushängt. In dem untersuchten Zeitfenster wurde
  kein Auswurf des physischen Backup-Ziellaufwerks nachgewiesen.
- Die installierte App ist 1.2.49. Der ursprüngliche Projekt-Checkout stand noch
  auf 1.2.45; die weitere Prüfung und Änderung basieren auf Commit `c47365e`
  aus dem Arbeitsverzeichnis der Version 1.2.49.

## Untersuchung des abgebrochenen Backups auf Backup03

Das Backup `20260921-142659` wurde ausschließlich gelesen und nicht verändert.

- 17 Teilarchive mit insgesamt 347.565.832.855 Byte sind in
  `.resume-state.jsonl` registriert; sämtliche vorhandenen Archivgrößen stimmen
  mit diesen Datensätzen überein.
- Darunter befinden sich das Parallels-Archiv (149,4 GB) und das Bilder-Archiv
  (165,5 GB). Das letzte registrierte Archiv ist `~/Library/LaunchAgents`,
  geschrieben um 04:08:05. Ein fertiges Preferences-Archiv ist nicht vorhanden.
- Alle 13 Archive unter 1 GB wurden erneut vollständig gelesen und ihre
  SHA-256-Prüfsummen mit dem Zwischenstand verglichen: alle stimmen überein.
- Die vier größeren Archive (Documents, Downloads, Parallels, Pictures) wurden
  bei dieser Untersuchung nicht erneut vollständig gelesen. Der Zwischenstand
  ersetzt keine neue vollständige Integritätsprüfung.
- `metadata.json` fehlt weiterhin; dies ist kein abgeschlossenes Backup.

## Reproduktion und Grenzen

Der vorhandene Integrationstest für Snapshot, Archivierung, Rücklesen,
Finalisierung und Test-Wiederherstellung bestand für Preferences sowohl mit
1.2.45 als auch mit 1.2.49. Zusätzlich bestand der Test mit Preferences aus dem
noch vorhandenen Original-Snapshot `com.apple.TimeMachine.2026-09-21-142706.local`
des fehlgeschlagenen Nachtlaufs (737 Einträge, 7.020.638 Dateibyte).

Diese begrenzten Tests verwendeten temporäre Ziele auf dem internen Datenträger.
Sie reproduzieren weder den mehrstündigen Gesamtlauf noch einen möglichen
gesperrten Sitzungszustand. Sie beweisen deshalb keine Behebung der Abbrüche.
Die Ursache des ursprünglichen Archivierungs-/Lesefehlers bleibt offen.

## Korrigierte Diagnoseanzeige

Bisher wurde ein Fehler oft erst an die Oberfläche zurückgegeben, nachdem
`PrivateDir::drop` temporäre Dateien entfernt hatte. Die aktive Phase wechselte
inzwischen auf „Temporäre Dateien aufräumen“. Die gedrosselte Entfernung großer
temporärer Dateien kann diese Verzögerung erheblich verlängern.

Archivierungs- und Datei-Zugriffsfehler werden jetzt am Fehlerort protokolliert,
bevor die temporären Ordner aufgeräumt werden. Dabei bleibt der ursprüngliche
Fehler unverändert erhalten. Archivfehler nennen Quelle und vorgesehenes Ziel.
Die Bereinigung nennt den temporären Ordner und meldet eigene Löschfehler
gesondert. Nach einem protokollierten Fehler heißt die Phase ausdrücklich
„Temporäre Dateien nach Fehler aufräumen“.

Diese Änderung verbessert die Diagnose; sie behauptet keine Reparatur der noch
nicht reproduzierten Ursache. Archivprüfung und Fehlerabbruch bleiben erhalten.

## Prüfung

Die vollständige Rust-Testsuite einschließlich neuer Regressionstests für die
Reihenfolge Fehler → Bereinigung bestand: 163 bestanden, 9 manuelle Tests
ausgelassen. Die Änderung wurde noch nicht als App-Update veröffentlicht.

## Nachtest mit der notarisierten Testversion 1.2.50

Der vollständige App-Lauf `20260922-132431` brach erneut bei Preferences ab.
Die neue Anzeige bestätigt die Reihenfolge: Archivierungsfehler um 18:45:38,
danach Bereinigung. Die App zeigt aktivierten Festplattenvollzugriff an.

Ein erneuter begrenzter Integrationstest mit einem frischen APFS-Snapshot,
Backup03 als Ziel und 40 MB/s bestand einschließlich Rücklesen, Finalisierung
und Wiederherstellung (738 Einträge, 7.010.301 Dateibyte). Dafür wurde ausschließlich
ein eigener temporärer `.macos-backup-probe-*`-Ordner angelegt und entfernt.
Separate Aufrufe von System-tar mit Zstandard sowie Gzip waren ebenfalls erfolgreich.

Das Systemprotokoll zeigt wiederholte Löschversuche von Time Machine gegen
den verwendeten Snapshot `com.apple.TimeMachine.2026-09-22-132433.local`.
**Diese Löschversuche scheiterten ausdrücklich mit „cannot delete mounted
snapshot“ und „Resource busy“.** Die begleitenden „Device not configured“-Meldungen
betreffen Löschversuche auf dem Snapshot-Mount. Daraus darf weder ein Verlust
der Quelle während des Laufs noch ein Auswurf des Backup-Ziellaufwerks abgeleitet
werden. Erst beim Abbruch hängt die App ihren eigenen Snapshot aus.

Der Snapshot dieses abgebrochenen Laufs war bei Beginn der Nachuntersuchung
nicht mehr verfügbar. Offen bleibt der kurze Preferences-Einzeltest innerhalb
der installierten App: Nur dieser kann den App-Ausführungskontext von der
mehrstündigen Laufzeit trennen. Die Ursache ist weiterhin nicht bewiesen.

Der vorhandene manuelle Integrationstest akzeptiert nun zusätzlich
`BACKUP_PROBE_TARGET` und `BACKUP_PROBE_MB_PER_S`, damit dieser gezielte Vergleich
reproduzierbar bleibt. Ohne diese Variablen bleibt sein bisheriges Verhalten erhalten.
