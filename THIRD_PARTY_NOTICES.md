# Third-party notices

macOS Backup Suite is licensed under the MIT License. That license applies to
the project's own source code; it does not replace the licenses of included
third-party components.

## Audit result — 22 September 2026

All 558 third-party crates resolved in the locked Cargo metadata were inspected. The
macOS normal dependency graph was additionally inspected with
`cargo tree --target aarch64-apple-darwin --edges normal`; the application and updater retain their own dependency graphs. The frontend runtime packages were inspected from
`package-lock.json` and their installed package manifests.

- Every inspected Rust crate declares an SPDX license expression; none is GPL,
  AGPL, SSPL, or another strong-copyleft license.
- The JavaScript runtime graph contains eight Tauri packages. Each is offered
  under MIT or Apache-2.0.
- The project can therefore be distributed under MIT. Notices and the terms of
  third-party components remain applicable to those components.

### Licenses present in the macOS Rust runtime

The resolved graph uses MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC,
Zlib, 0BSD, CC0-1.0, Unlicense, Unicode-3.0, and MPL-2.0, including compatible
dual-license combinations. For dual-licensed components, distribution may use
the MIT alternative where offered.

The five MPL-2.0 components are `cssparser`, `cssparser-macros`, `dtoa-short`,
`option-ext`, and `selectors`. They are included unmodified; MPL-2.0's
file-level obligations apply to those components, while the Suite's own code
may remain MIT-licensed.

### Direct runtime components

| Component | Version | License |
| --- | ---: | --- |
| tauri | 2.11.2 | Apache-2.0 OR MIT |
| tauri-plugin-dialog | 2.7.1 | Apache-2.0 OR MIT |
| tauri-plugin-fs | 2.5.1 | Apache-2.0 OR MIT |
| tauri-plugin-notification | 2.3.3 | Apache-2.0 OR MIT |
| tauri-plugin-opener | 2.5.4 | Apache-2.0 OR MIT |
| tauri-plugin-shell | 2.3.5 | Apache-2.0 OR MIT |
| tauri-plugin-store | 2.4.3 | Apache-2.0 OR MIT |
| tauri-plugin-updater | 2.11.0 | Apache-2.0 OR MIT |
| serde | 1.0.228 | MIT OR Apache-2.0 |
| serde_json | 1.0.149 | MIT OR Apache-2.0 |
| chrono | 0.4.44 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| walkdir | 2.5.0 | Unlicense OR MIT |
| dirs | 5.0.1 | MIT OR Apache-2.0 |
| libc | 0.2.186 | MIT OR Apache-2.0 |
| unicode-normalization | 0.1.25 | MIT OR Apache-2.0 |
| xattr | 1.6.1 | MIT OR Apache-2.0 |
| @tauri-apps/api | 2.11.0 | Apache-2.0 OR MIT |
| @tauri-apps/plugin-dialog | 2.7.1 | MIT OR Apache-2.0 |
| @tauri-apps/plugin-fs | 2.5.1 | MIT OR Apache-2.0 |
| @tauri-apps/plugin-notification | 2.3.3 | MIT OR Apache-2.0 |
| @tauri-apps/plugin-opener | 2.5.4 | MIT OR Apache-2.0 |
| @tauri-apps/plugin-shell | 2.3.5 | MIT OR Apache-2.0 |
| @tauri-apps/plugin-store | 2.4.3 | MIT OR Apache-2.0 |
| @tauri-apps/plugin-updater | 2.11.0 | MIT OR Apache-2.0 |

### AppleArchive and LZFSE

Backup, verification and restore invoke `/usr/bin/aa`, provided by macOS.
AppleArchive/LZFSE are operating-system components, not bundled or relicensed
under the project's MIT license. No separate compressor is distributed.
The direct TAR/flate2 backup dependencies and local TAR patch are no longer used.
The Tauri application updater still uses archive/compression dependencies for
application-update packages; this is separate from the `.aar` backup format.

For a new release, repeat this audit after every dependency update. This
document records license metadata supplied by dependencies; it is not a legal
opinion.
