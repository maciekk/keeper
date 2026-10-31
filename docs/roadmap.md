# Milestones

## Implemented: genesis + read-only full-byte comparison

The original Stage 0/1 design is made concrete in [catalog-v1.md](catalog-v1.md).
We also implement full-byte status comparison now, rather than creating a catalog
that cannot yet be exercised end to end. `status` is explicitly expensive; no
ambiguous `verify`, `scan`, or `scrub` aliases exist yet.

Tests cover byte-level filename escaping, duplicate objects and hard-linked
copies, byte changes with preserved size/mtime, baseline immutability, no-replace
publication, writer exclusion, failed-init retry, invalid manifests/tables, missing
catalogs, and refusal of symlinks/special objects.

## Next: make verification usable on large real datasets

Before running repeatedly over the recovered DATA corpus:

- Add progress and throughput reporting on stderr without corrupting reports.
- Define `status` (eventually incremental) versus `scrub` (always physical reads)
  before introducing metadata shortcuts. Preserve today's explicit semantics until
  that transition is documented and tested.
- Handle clean cancellation and resumable **per-path physical verification**.
  Content-level validation may be shared; reading one duplicate does not prove the
  bytes of another copy. Decide whether verification times are disposable cache
  or durable history, rather than conflating them with immutable object identity.
- Test I/O failures, cancellation, and process termination at publication points.
  Current unit tests are not proof of power-loss durability on actual hardware.
- Add deterministic mutation-during-read tests and test supported target filesystems.
- Benchmark traversal/memory on a copy of representative data before tuning workers.

## Then: explicit acceptance

Do not implement baseline updates as an overwrite of `paths.tsv`.

- Specify review/accept semantics for additions, deletions, changes and renames.
- Design multi-generation atomic replacement, recovery, locking/read consistency,
  and generation/version compatibility.
- Revalidate observed bytes before acceptance; never promote a stale report.
- Preserve the existing baseline on every rejected or failed acceptance.

## Later: semantic validation and backup comparison

- Separate known bytes from structural validity. Invoke validators directly,
  without a shell; bound execution time and capture diagnostic output safely.
- Add validation records keyed by full object hash plus validator/version identity.
- Design resumable checkpoints and age/budget scheduling.
- Compare copies without conflating path identity, content identity and physical
  replica trust. Repair, copying, and write-through import remain separate work.

The wider [design](design.md) describes these ambitions. The DATA-storage policy's
trust-ranking and repair wishes do not expand this milestone into a backup or
recovery engine.
