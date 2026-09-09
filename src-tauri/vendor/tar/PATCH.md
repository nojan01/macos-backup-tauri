Vendored tar 0.4.46 (https://github.com/composefs/tar-rs), under the included MIT/Apache-2.0 licenses.

Local change: src/pax.rs parses PAX records using their decimal byte length rather than splitting on newline bytes. This preserves binary SCHILY xattrs and multiline paths. Invalid lengths, truncation, missing terminators and missing/empty keys still fail closed. Unit tests cover binary values, subsequent records, invalid lengths and digit boundaries. Application tests cover native macOS backup/readback/restore.

Keep this patch until an upstream version provides equivalent parsing and passes these regressions. No archive format or metadata exclusions change.
