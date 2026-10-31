# Milestones

## Implemented: genesis + read-only full-byte status

The original Stage 0/1 design is made concrete in [catalog-v1.md](catalog-v1.md).
`status` is explicitly expensive; no ambiguous `verify`, `scan`, or `scrub` aliases
exist yet. No commands accept changed bytes, repair files, or replace a catalog.

## Implemented: trustworthy replica comparison primitives

- `compare LEFT RIGHT`: path-sensitive recorded-catalog comparison, with an
  explicit warning that current dataset bytes were not read.
- `compare --live LEFT RIGHT`: physically read each tree without using or changing
  catalogs. Both metadata preflights complete before hashing either tree.
- `preflight`: metadata-only inventory accumulating traversal/unsupported-object
  issues without reading file contents or creating metadata. It does not guarantee
  subsequent file readability or filesystem publication support.
- Progress and read throughput on stderr, including inside large files.
- Deterministic tests for mutation during read, namespace changes since preflight,
  failed/short/long/interrupted reads, and process death before/after publication.
- Golden format-1 fixture captured from the foundation release, confirming existing
  catalogs remain usable without reindexing and new serialization is unchanged.

Other tests cover byte-level filename escaping, duplicate objects and hard-linked
copies, baseline immutability, no-replace publication, writer exclusion, failed-init
retry, malformed catalogs, comparison scope, and refusal of symlinks/special objects.

## Next: resumable large-drive operation

Before the recommended definitive DATA/DATA2 run:

- Handle clean cancellation and resumable initialization/verification with durable
  checkpoints. A successful physical pass must not depend on an unchecked cache.
- Define a scan/session identity, root identity, checkpoint version, and conditions
  for invalidating/rechecking completed work on resume. File contents can change
  without size/mtime changes; never describe reused metadata as a fresh full scrub.
- Track **per-path physical verification**. Content-level validation may be shared;
  reading one duplicate does not prove the bytes of another copy. Decide whether
  verification times are disposable cache or durable history, rather than conflating
  them with immutable object identity.
- Test actual target filesystems on disposable copies: locks, sync, no-replace
  publication and failure behavior. Existing process-kill tests do not establish
  actual power-loss durability or target-device compatibility.
- Benchmark traversal, metadata inventory memory, and hashing on representative
  samples. Current inventories/catalogs are in RAM, including both trees during
  live comparison. Avoid worker tuning before measurements.
- Decide how to select corpus scope when a drive root contains unsupported system
  objects. No implicit exclusions, dangerous ignore defaults, or quiet partial
  success. Roots, inclusion policy and comparison scope must remain explicit.

Keep `status` as an explicit full-byte operation until an incremental `status`
versus physical `scrub` distinction is documented and tested. Preserve format-1
read compatibility; schema-only upgrades must not force a data rehash.

## Then: explicit acceptance

Do not implement baseline updates as an overwrite of `paths.tsv`.

- Specify review/accept semantics for additions, deletions, changes and renames.
- Design multi-generation atomic replacement, recovery, locking/read consistency,
  and generation/version compatibility, including lossless format migrations.
- Revalidate observed bytes before acceptance; never promote a stale report.
- Preserve the existing baseline on every rejected or failed acceptance.

## Later: semantic validation and richer backup comparison

- Separate known bytes from structural validity. Invoke validators directly,
  without a shell; bound execution time and capture diagnostic output safely.
- Add validation records keyed by full object hash plus validator/version identity.
- Design resumable checkpoints and age/budget scheduling.
- Add object-presence backup comparison separately from path-exact replica checks.
  Do not conflate path identity, content identity and physical replica trust.
  Repair, copying, and write-through import remain separate work.

The wider [design](design.md) describes these ambitions. The DATA-storage policy's
trust-ranking and repair wishes do not expand these milestones into a backup or
recovery engine. See [the drive workflow](replica-workflow.md) for staged testing.
