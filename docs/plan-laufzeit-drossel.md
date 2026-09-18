# Plan – Durchsatzbegrenzung zur Laufzeit ändern (geplant für 1.2.50 oder später; ursprünglich als 1.2.47 vorgesehen, 1.2.47–1.2.49 wurden für Hotfixes vergeben)

## Ziel
Während Backup, Prüfung und Wiederherstellung soll das MB/s-Limit live geändert
(oder ein-/ausgeschaltet) werden können, ohne den Lauf abzubrechen. Wirkung
innerhalb von < 1 s. Der zuletzt gewählte Wert kann ins Profil übernommen werden.

## Warum keine Temperaturregelung
- Der USB-NVMe-Bridge-Chip hat keinen per Software lesbaren Sensor.
- `smartctl -d sntrealtek/sntasmedia/sntjmicron` liefert nur die NVMe-Temperatur
  (Proxy, träge, gleiche Gehäusewärme) und braucht auf macOS `sudo` → für eine
  App ohne Root-Helfer ungeeignet.
- Für den Bridge-Chip zählt letztlich die mittlere Datenrate; ein konstantes
  Limit ist thermisch günstiger als Vollgas mit Pausen. Deshalb: manuelles
  Nachregeln + Lernen aus dem Ergebnis statt Regelung nach Sensor.

## Backend (`src-tauri/src`)

### throttle.rs
- `Bucket::set_rate(bytes_per_sec, now)`: `refill(now)` zuerst, dann Rate
  setzen; Tokens auf neue Kappe (`rate × BURST`) begrenzen, negative Schuld
  proportional umrechnen (`tokens *= new/old`), damit ein Sprung von 20 → 200
  MB/s nicht sekundenlang nachwartet.
- `Active.mb_per_s` → `Arc<AtomicU32>`; `Limiter.mb_per_s()` liest daraus
  (Fortschrittstext „gedrosselt auf N MB/s" folgt sofort).
- `pub(crate) fn set_rate(mb_per_s: Option<u32>) -> Result<RateChange, String>`
  - `Some(n)`: `validate_mb_per_s`, Bucket anpassen, Atomic setzen.
  - `None`: Limit aufheben → `Active.unlimited = AtomicBool(true)`;
    `Limiter::acquire`/`ChildGovernor::tick` prüfen das Flag und geben frei
    (Governor: `resume()`), ohne den Guard zu entfernen. Erneutes `Some(n)`
    schaltet wieder ein. Kein Neu-Aktivieren nötig (`activate` bleibt
    ausschließlich am Laufstart).
  - Fehler `"Keine Durchsatzbegrenzung aktiv"` wenn kein Lauf läuft oder das
    Profil ohne Limit gestartet wurde (dann hat der Lauf keinen Guard →
    Einschalten zur Laufzeit ist nicht möglich; UI zeigt das an).
  - Rückgabe `RateChange { old: Option<u32>, new: Option<u32> }` für das Log.
- `describe()` liest den Atomic.

### lib.rs
- Neuer Command `set_throttle_rate(mb_per_s: Option<u32>, persist: bool,
  window)`:
  1. `throttle::set_rate(...)`.
  2. Log-Zeile in den laufenden Log-Kanal (backup-log / restore-log; welcher
     aktiv ist, steht bereits über den laufenden Impl-Kontext fest → kleines
     `static ACTIVE_LOG_EVENT: Mutex<Option<&'static str>>`, gesetzt in
     `create_backup_impl`/`verify_*_impl`/`restore_items_impl` neben
     `throttle::activate`): „🌡️ Durchsatzbegrenzung geändert: 80 → 40 MB/s"
     bzw. „… aufgehoben" / „… wieder aktiv: 40 MB/s".
  3. `persist`: Profil laden, `throttle_mb_per_s`/`throttle_enabled` setzen,
     über bestehende `save_config`-Logik schreiben.
- Registrierung in `generate_handler!`.
- Beim Laufstart optional: Wenn der **vorherige Lauf auf dasselbe Ziel-Volume**
  mit „Gerät nicht mehr erreichbar"/ENODEV/EIO abgebrochen ist, Hinweis im Log
  „Letzter Lauf auf diesem Ziel endete mit Gerätefehler – Limit senken?"
  (nur Hinweis, keine Automatik; Erkennung über bestehende Fehlertexte im
  Backup-Ergebnis, gespeichert pro Profil als `last_target_failure: Option<
  String>`). → als optionaler Teilschritt, kann entfallen.

### Timeout-Korrektur
- `run_child` rechnet Pausenzeit bereits heraus; bei Aufhebung des Limits
  bleibt die bisherige `paused_total` erhalten – nichts zu tun.

## Frontend (`src/`, `index.html`)
- In der `progress-section` ein neues, nur während eines Laufs sichtbares
  Bedienfeld `#throttle-live` (unter dem Balken):
  `🌡️ <input type=range min=1 max=500 step=1> <input type=number> MB/s
  [☐ ohne Limit] [Im Profil speichern]`
  - Range-Skala logarithmisch (1…5000) gemappt, Zahlenfeld exakt.
  - Debounce 250 ms → `invoke("set_throttle_rate", { mbPerS, persist:false })`.
  - „Im Profil speichern" → `persist:true`, danach Häkchen kurz bestätigen.
  - Sichtbar nur, wenn der Lauf mit aktivem Limit gestartet wurde (Backend
    sendet mit dem Start-Log „Durchsatzbegrenzung aktiv" ein Event
    `throttle-state {active:true, mbPerS}`; bei Laufende `active:false`).
  - Bei `active:false` (Profil ohne Limit): Feld ausgeblendet, Tooltip im
    Einstellungsdialog erklärt, dass Einschalten nur vor dem Start geht.
- `src/throttle-ui.ts`: `sliderToMbPerS`, `mbPerSToSlider` (log-Mapping),
  `formatRateChange`.
- i18n de/en für Labels; `messages.ts`-Paare für die neuen Log-Zeilen
  (`'🌡️ Durchsatzbegrenzung geändert: {0} → {1} MB/s'`, `'… aufgehoben'`,
  `'… wieder aktiv: {0} MB/s'`, `'Keine Durchsatzbegrenzung aktiv'`).
- Fortschrittstext bleibt „… MiB geschrieben · gedrosselt auf N MB/s" und
  folgt dem neuen Wert automatisch.

## Tests
- Rust (`throttle.rs`): `set_rate_halves_and_doubles_wait` (Wartezeit skaliert
  mit Rate), `set_rate_rescales_debt_on_increase`, `unlimited_releases_waiters`
  (Thread hängt in `acquire`, `set_rate(None)` lässt ihn binnen SLICE frei),
  `set_rate_without_active_run_errors`.
- Rust (`governor_tests`): Governor mit 1 MB/s pausiert, `set_rate(None)` →
  Kind wird per SIGCONT fortgesetzt und `paused_since` gelöscht.
- Rust (`backup/tests.rs`): Roundtrip, bei dem mitten im Lauf die Rate
  geändert wird (Thread ruft nach 200 ms `set_rate(Some(5000))`), Archiv
  vollständig und korrekt.
- Frontend (`tests/throttle.test.mjs`): log-Mapping Round-Trip, Grenzen,
  `formatRateChange`; `tests/messages.test.mjs` prüft neue Paare automatisch.

## Version & Doku
- 1.2.46 → 1.2.47 in package.json, package-lock.json (×2), tauri.conf.json,
  Cargo.toml, Cargo.lock.
- `releases/1.2.47.md`; README-Abschnitt „Durchsatzbegrenzung" ergänzen
  (Laufzeit-Regler, Hinweis warum keine Temperaturregelung).

## Release-Ablauf (Lehre aus 1.2.46)
- Updater-Archiv nach dem Stapeln mit
  `COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata` erzeugen (siehe PR #2)
  oder besser: kleines `scripts/release-macos.sh` wie im BurnISO-Projekt, das
  Build → Notarisierung (.app) → Staple → Updater-Archiv → DMG-Neubau →
  DMG-Notarisierung → Manifest → `gh release` in einem Lauf erledigt.

## Aufwand
Backend ~150 Zeilen, Frontend ~120 Zeilen, Tests ~150 Zeilen. Etwa ein halber
Arbeitstag inklusive Release.
