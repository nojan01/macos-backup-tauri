# Segmented native AppleArchive container, version 1

New backups use `.aarset`; the extension is descriptive, detection uses magic bytes.
The outer backup metadata continues to record the SHA-256 and byte length of this
single immutable file. Hardlink reuse, atomic publication and verification receipts
therefore retain their existing regular-file semantics. Alpha 1 `.aar` files remain
readable through the native path.

## Layout

1. Eight literal bytes: `MBS2AA01`.
2. Consecutive native LZFSE AppleArchive streams. Each independently decodes to
   exactly one regular file named `payload`, with no hardlink cluster.
3. UTF-8 JSON index: `version: 1`, `raw_bytes`, and an ordered `parts` array.
   Each part records `offset`, `compressed`, `raw`, `compressed_sha256`, and
   `raw_sha256`. Sizes and offsets are unsigned byte counts; hashes are lowercase
   hexadecimal SHA-256 strings. No gaps, overlaps or unindexed bytes are allowed.
4. The 32 raw bytes of the JSON index's SHA-256.
5. The JSON length as an eight-byte little-endian unsigned integer.
6. Eight literal bytes: `MBS2END1`.

A raw part contains at most 1,073,741,824 bytes. A compressed part is limited to
that size plus 16 MiB; the index is limited to 64 MiB. Nested `.aarset` parts are
rejected. Attribute and ACL blobs of the wrapper payload are separately bounded.
The concatenation of the decoded `payload` files is the original **uncompressed
native AppleArchive stream**, including its original headers and all data blobs.
Boundaries may split headers, file contents, resource forks or other attributes.
No entire source file has to fit in a part.

## Creation and verification

The archiver emits native raw bytes. A tee writes them into bounded parts while
Apple's `AAHeader` API parses each header. File data and XAT attribute values are
hashed incrementally against the immutable source manifest. ACLs are applied to
an owned empty metadata probe and canonicalized with the existing ACL reader.
Paths, types, modes, modification times, flags, symlink targets and hardlink groups
are checked; missing, extra and colliding entries fail the operation. Embedded SH2
hashes are checked against the actual file bytes as well. Ownership is intentionally
mapped to the restoring user, as in the native Alpha 1 format.

Each encoded part is appended to the unpublished target, then read back from that
same open file descriptor. Its encoded hash is checked before decoding; the decoded
payload's length and hash must match the original raw bytes. The raw spool and
verification copy are then discarded. The original stream-to-source comparison and
the destination-to-stream comparison together cover every stored source byte.
Only after all parts, the index/footer, and the final source guard pass is the
container atomically published. Failure or cancellation removes only private staging.
A screen-lock-related permission denial retains the existing wait-for-unlock policy;
a fresh private container is used for a retry.

The work area reserves 5 GiB on the system temporary volume. Target preflight remains
conservative (archive budget, entry overhead, 10% margin, bounded workspace and 8 GiB
reserve). Capacity is rechecked before each part. Reads and writes on the protected
target share the existing throughput limiter. Readback uses normal filesystem I/O;
it does not assert that operating-system or device caches were bypassed.

## Restore and compatibility

A reader validates index bounds and ordering, then verifies and decodes one part at
a time. It removes the previous temporary part before loading the next and streams
the reconstructed native archive to `aa list` / `aa extract`. The existing validated
index, private extraction and safe merge workflow applies. A real restore or test
restore still requires enough destination space for all selected restored files.
Standalone Alpha 1 native archives retain their original full-readback verification
path. New backups never write the old format; they use the distinct `.aarset` name.
