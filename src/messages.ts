/** Translate complete application templates. Captured paths and names stay verbatim. */
export const messagePairs: [string,string][] = [
  ["Sprache: Deutsch", "Language: English"],
  ["{0} beschreibbare Volumes gefunden (Time Machine ausgeschlossen).", "{0} writable volumes found (Time Machine excluded)."],
  ["♻️  Resume-Modus: {0} bereits gesicherte Einträge aus {1}", "♻️  Resume mode: {0} previously saved items from {1}"],
  ["⏭️  Keine Änderungen in {0} – Archiv übernommen aus {1}", "⏭️  No changes in {0} – archive reused from {1}"],
  ["⚠️  Archiv-Wiederverwendung fehlgeschlagen für {0} – erstelle neu", "⚠️  Archive reuse failed for {0} – creating a new archive"],
  ["Archiviere {0}...", "Archiving {0}..."],
  ["❌ Konnte Backup-Inhalt nicht lesen: {0}", "❌ Could not read backup contents: {0}"],
  ["❌ Ordner-Auswahl fehlgeschlagen: {0}", "❌ Folder selection failed: {0}"],
  ["✅ Test-Restore OK: {0} Dateien, {1}", "✅ Test restore OK: {0} files, {1}"],
  ["   📁 Entpackt nach: {0}", "   📁 Extracted to: {0}"],
  ["❌ Test-Restore Fehler: {0}", "❌ Test restore error: {0}"],
  ["Backup vollständig; Aktualisierung der Latest-Verknüpfung fehlgeschlagen: {0}", "Backup complete; updating the latest link failed: {0}"],

  ['Backup wird vorbereitet','Preparing backup'],
  ['Software-Inventar erfassen','Collecting software inventory'],
  ['Unveränderlichen APFS-Sicherungsstand erstellen','Creating an immutable APFS snapshot'],
  ['Zugriff auf alle Quelldateien prüfen','Checking access to all source files'],
  ['Quelldateien lesen und prüfen: {0}','Reading and checking source files: {0}'],
  ['Quelländerungen prüfen: {0}','Checking source changes: {0}'],
  ['Archiv erstellen und komprimieren: {0}','Creating and compressing archive: {0}'],
  ['Archivstruktur und Kompression prüfen','Checking archive structure and compression'],
  ['Archiv-Dateiinhalte im Datenstrom prüfen','Verifying archived file contents as a stream'],
  ['Rückgelesene Dateiattribute prüfen: {0}','Verifying restored file attributes: {0}'],
  ['Rückgelesene Dateiinhalte prüfen: {0}','Verifying restored file contents: {0}'],
  ['Archiv entpacken / macOS-Dateiattribute und Rechte setzen','Extracting archive / restoring macOS attributes and permissions'],
  ['Archiv entpacken / macOS-Metadaten setzen','Extracting archive / restoring macOS metadata'],
  ['Gesicherte macOS-Dateiflags wiederherstellen','Restoring saved macOS file flags'],
  ['Temporäre Rücklesedaten aufräumen','Cleaning up temporary readback data'],
  ['Temporäre Dateien aufräumen','Cleaning up temporary files'],
  ['Archiv-Prüfsumme berechnen','Calculating archive checksum'],
  ['Bereits geprüftes Archiv für Fortsetzung prüfen: {0}','Checking previously verified archive for resume: {0}'],
  ['Backup pausiert: Mac entsperren, um geschützte Dateien weiterzulesen','Backup paused: unlock the Mac to continue reading protected files'],
  ['Starte Backup-Vorbereitung...','Starting backup preparation...'],
  ['Prüfe Verfügbarkeit aller ausgewählten Quellen …','Checking availability of all selected sources …'],
  ['Scanne Quellverzeichnisse...','Scanning source folders...'],
  ['Scanne {0} ({1}/{2})','Scanning {0} ({1}/{2})'],
  ['Initialisiere Backup...','Initializing backup...'],
  ['App-Einstellungen: {0}','App settings: {0}'],
  ['Konsistenter Dateistand: {0}. Geöffnete und weiter bearbeitete Originaldateien beeinflussen dieses Backup nicht.','Consistent source snapshot: {0}. Opening or editing original files does not affect this backup.'],
  ['Dateien werden aus {0} gesichert; Änderungen an den Originalen beeinflussen diesen Sicherungsstand nicht.','Files are backed up from {0}; changes to the originals do not affect this snapshot.'],
  ['Dateiinhalte werden vollständig im Datenstrom geprüft; nur Dateiattribute benötigen begrenzten temporären Speicher.','File contents are fully verified as a stream; only file attributes require bounded temporary storage.'],
  ['Abschlussprüfung: eingefrorener Sicherungsstand und Archive werden erneut geprüft …','Final check: verifying the frozen source snapshot and archives again …'],
  ['Laufzeit-Socket übersprungen (keine Dateidaten): {0}','Runtime socket skipped (no file data): {0}'],
  ['{0} Laufzeit-Sockets übersprungen; vollständige Liste: skipped-runtime-sockets.json','{0} runtime sockets skipped; full list: skipped-runtime-sockets.json'],
  ['Fortsetzung: {0} vorhandene Archive werden mit dem aktuellen Snapshot und ihrer Prüfsumme verglichen; veränderte Quellen werden neu gesichert.','Resuming: checking {0} existing archives against the current snapshot and their checksums; changed sources will be backed up again.'],
  ['✅ Fortgesetzt: {0} unverändert, bereits rückgelesenes Archiv mit SHA-256 bestätigt','✅ Resumed: {0} unchanged, previously verified archive confirmed with SHA-256'],
  ['🔁 Inkrementeller Modus aktiv (Basis: {0})','🔁 Incremental mode active (base: {0})'],
  ['🔁 Inkrementeller Modus aktiv (kein Basis-Backup gefunden — Vollbackup)','🔁 Incremental mode active (no base backup found — full backup)'],
  ['Datei während des Lesens geändert; erneuter Versuch {0}/3: {1}','File changed while reading; retry {0}/3: {1}'],
  ['Snapshot-Einhängepunkt bleibt erhalten: {0} ({1})','Snapshot mount point retained: {0} ({1})'],
  ['{0}: {1} vorhandene Einstellungspfade ausgewählt (bereits enthaltene Pfade werden nicht doppelt ergänzt).','{0}: selected {1} existing settings paths (paths already included are not added twice).'],
  ['Archiviere {0} ...','Archiving {0} ...'],
  ['Homebrew-Cache prüfen...','Checking Homebrew cache...'],
  ['Homebrew-Cache archivieren ({0} MB)...','Archiving Homebrew cache ({0} MB)...'],
  ['✅ Homebrew-Cache archiviert: {0} MB','✅ Homebrew cache archived: {0} MB'],
  ['⚠️ Kein lokaler Homebrew-Cache gefunden – übersprungen; Option bleibt für künftige Backups aktiv.','⚠️ No local Homebrew cache found – skipped; the option remains enabled for future backups.'],
  ['Safari-Einstellungen sichern...','Backing up Safari settings...'],
  ['✅ Safari-Einstellungen archiviert: {0} Dateien/Ordner','✅ Safari settings archived: {0} files/folders'],
  ['=== Backup gestartet: {0} ===','=== Backup started: {0} ==='],
  ['=== Backup beendet: {0} (Dauer: {1}) ===','=== Backup finished: {0} (Duration: {1}) ==='],
  ['Backup abgeschlossen.','Backup completed.'],
  ['⚠️ Backup abgebrochen!','⚠️ Backup cancelled!'],
  ['Nicht genug Speicherplatz','Insufficient disk space'],
  ['Speicherplatzprüfung: {0} GB frei, ~{1} GB neu/geändert (benötigt ≥ {2} GB mit begrenzter Reserve)','Free space check: {0} GB free, ~{1} GB new/changed (need ≥ {2} GB with bounded reserve)'],
  ['Keine Dateien im Backup zum Verifizieren.','No files in the backup to verify.'],
  ['Verifiziere {0}/{1}: {2}','Verifying {0}/{1}: {2}'],
  ['{0}/{1} Dateien verifiziert','{0}/{1} files verified'],
  ['Alle {0} Dateien erfolgreich verifiziert!','All {0} files verified successfully!'],
  ['{0} von {1} Dateien fehlerhaft','{0} of {1} files failed'],
  ['✅ App-Installer kopiert: {0}','✅ App installer copied: {0}'],
];
const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
// Prefer specific templates over generic prefixes such as "Archiving {0}...".
messagePairs.sort((a,b) => b.map(s=>s.replace(/\{\d+\}/g, "").length).reduce((x,y)=>x+y,0) - a.map(s=>s.replace(/\{\d+\}/g, "").length).reduce((x,y)=>x+y,0));
const compiled = messagePairs.map(pair => pair.map(template => new RegExp('^' + template.split(/\{\d+\}/).map(escape).join('([\\s\\S]*?)') + '$')));
export function localizeMessage(message: string, language: string): string {
  const target = language === 'en' ? 1 : 0;
  const source = 1-target;
  // Heartbeats consist of a phase, elapsed time and optional typed counters.
  const heartbeat=/^(.+?) · (\d+:\d{2} min)(?: · ([\s\S]*))?$/.exec(message);
  if (heartbeat) {
    let detail=heartbeat[3] ?? '';
    const waitFrom=source===0?'warte auf Abschluss dieses Arbeitsschritts':'waiting for this step to finish';
    const waitTo=target===0?'warte auf Abschluss dieses Arbeitsschritts':'waiting for this step to finish';
    let waiting=false;
    if (detail===waitFrom) {detail='';waiting=true;}
    else if (detail.endsWith(' · '+waitFrom)) {detail=detail.slice(0,-waitFrom.length-3);waiting=true;}
    const patterns: [RegExp,string][] = target===1 ? [
      [/^(\d+) Einträge · ([\d.]+) MiB gelesen · ([\d.]+) MiB\/s · ([\s\S]*)$/, '$1 entries · $2 MiB read · $3 MiB/s · $4'],
      [/^([\d.]+) MiB gelesen$/, '$1 MiB read'],
      [/^(\d+) Pfade geprüft · ([\s\S]*)$/, '$1 paths checked · $2'],
      [/^(\d+) Pfade auf Zugriff geprüft$/, '$1 paths checked for access'],
    ] : [
      [/^(\d+) entries · ([\d.]+) MiB read · ([\d.]+) MiB\/s · ([\s\S]*)$/, '$1 Einträge · $2 MiB gelesen · $3 MiB/s · $4'],
      [/^([\d.]+) MiB read$/, '$1 MiB gelesen'],
      [/^(\d+) paths checked · ([\s\S]*)$/, '$1 Pfade geprüft · $2'],
      [/^(\d+) paths checked for access$/, '$1 Pfade auf Zugriff geprüft'],
    ];
    for (const [pattern,replacement] of patterns) if (pattern.test(detail)) {detail=detail.replace(pattern,replacement);break;}
    detail=localizeMessage(detail,language);
    return [localizeMessage(heartbeat[1],language),heartbeat[2],detail,waiting?waitTo:''].filter(Boolean).join(' · ');
  }
  for (let i=0; i<messagePairs.length; i++) {
    const match=compiled[i][source].exec(message);
    if (match) return messagePairs[i][target].replace(/\{(\d+)\}/g,(_,n) => match[Number(n)+1]);
  }
  return message;
}
