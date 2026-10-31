# Keeper

A Git-like integrity catalog for ordinary filesystem data. Keeper records which
BLAKE3 content object belongs at each path. Its authoritative metadata is plain
text, not an opaque database. It does not store file contents or replace backups.

This is the Rust rewrite of `maciekk/keeper`. The unused Go experiment is preserved
in Git history (last Go commit: `5d021b2e818bda9e260432b67ac323a22844064e`).

## Current milestone: resumable operations (target-drive qualification pending)

**Early development, not yet a production maintenance tool.** Test on a small copy
first. Do not use the existence of a Keeper catalog as permission to delete originals
or backups. No commands modify dataset files, accept changed content, copy, or repair.

Implemented:

- `init`: hash regular files and publish a self-checked genesis catalog.
- `catalog check`: verify catalog checksums, canonical encoding, and consistency.
- `status`: physically reread **every regular file**, reporting added, missing,
  and changed paths. No metadata shortcut and no baseline updates.
- `init --resume` / `status --resume`: checkpointed multi-invocation sessions,
  cooperative SIGINT/SIGTERM cancellation, and validated per-path reuse. Explicitly
  **not fresh full scrubs**. See [resume semantics and recovery](docs/resume.md).
- `preflight`: metadata-only inventory listing unsupported objects and traversal
  errors. Automatically runs before any hashing; live comparison preflights both
  trees before hashing either.
- `compare LEFT RIGHT`: compare recorded path/content catalogs, not current bytes.
- `compare --live LEFT RIGHT`: physically read both trees and compare current
  paths/content, without requiring, using, or updating catalogs.
- In-place progress and throughput on stderr, including within large files,
  percentage and estimated time remaining when totals are known. Use
  `--progress auto|always|never` (default: auto, enabled only on a terminal).
  Forced redirected progress logs phase starts/completions without terminal controls
  or repetitive updates.
- Human-friendly numbers: comma-separated counts through 999,999, then compact
  M/G/T-style suffixes. Byte sizes always use B/KiB/MiB/GiB/etc., without raw-byte
  totals. Exact values remain in catalog metadata. File/unique-content summaries
  provide context. Restrained colour highlights progress phases, completion,
  values, and report labels; automatic colour checks stdout and stderr separately.
  `NO_COLOR` disables automatic colour. Override with `--color auto|always|never`.
- Discoverable command syntax in `--help`; `help` is equivalent, and
  `help <command>` (including `help catalog check`) provides focused details.
- Full BLAKE3 IDs, reversible Unix filename encoding, deduplicated object records.
- Exclusive writer locking, synced staging, atomic no-replace publication.
- Fail closed on read errors, detected concurrent mutations, unsupported filesystem
  objects, and invalid catalogs. No partial-success status report.

Deferred: explicit acceptance, general incremental scans, rename inference, semantic
validators, durable verification history, object-presence-only backup
comparison, and performance tuning. Renames currently appear as a missing path plus
an added path. Comparison is deliberately path-sensitive: identical bytes elsewhere
do not establish an exact replica. Format 1 is unchanged from the foundation;
existing catalogs require no conversion or reindexing.

## Why Rust?

Keeper is intentionally both a useful integrity tool and a **learn-Rust project**.
Rust also fits the job on practical grounds:

- Systems-level control, strong filesystem APIs, and low runtime overhead.
- Excellent BLAKE3 support for large-data hashing.
- Explicit control of concurrency and resource use, including different HDD/SSD needs.
- Strong support for directly invoking external semantic validators without a shell.
- Distribution as a single executable rather than an application runtime stack.

Prefer a straightforward synchronous pipeline with controlled worker pools. Do not
introduce async or other machinery just to learn it; require measured need. Rust is
not a substitute for a correct filesystem protocol, failure testing, or independent
backups. See [the design principles](docs/principles.md) for those commitments.

## Build and try

Requires Linux and a recent stable Rust toolchain (edition 2024). Run from this
checkout:

```sh
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings

./target/release/keeper --help
./target/release/keeper help compare
./target/release/keeper help catalog check

# Use an existing dataset directory, not the Keeper source checkout.
./target/release/keeper preflight /path/to/small-test-dataset
./target/release/keeper init /path/to/small-test-dataset
./target/release/keeper catalog check /path/to/small-test-dataset
./target/release/keeper status /path/to/small-test-dataset
```

The dataset root defaults to the current directory. Keeper does not search parent
directories for a catalog. Roots may be explicitly chosen through a symlink; the
resolved dataset root is used throughout.

Exit codes: **0** success/no differences; **1** completed status/comparison with differences;
**2** operational error, unsupported object, or invalid catalog. CLI usage errors
also return 2. File paths in catalog rows and status reports use the same escaped
ASCII notation, so control characters cannot forge additional report lines.

`init` records the bytes currently present. **It does not establish that those bytes
are good or format-valid.** Review/validate your sources independently. A hash match
means unchanged bytes, not successful JPEG decoding or a valid archive.

## Comparing replicas

```sh
# Compare the previously recorded catalogs. Does not examine current data bytes:
./target/release/keeper compare /path/to/left /path/to/right

# Freshly hash both trees. No catalog required or created:
./target/release/keeper compare --live /path/to/left /path/to/right

# For progress in a redirected/noninteractive run:
./target/release/keeper compare --live --progress always \
  /path/to/left /path/to/right >comparison.txt 2>progress.log
```

Reports explicitly state their scope. Differences use `ONLY_LEFT`, `ONLY_RIGHT`,
and `DIFFERENT` records; different files include both full hashes and sizes. Errors
return 2, not a misleading match or a partial-success comparison. Roots must be
distinct and non-overlapping after symlink resolution; mount aliases/bind mounts
are not fully detected, so choose genuinely separate datasets.

**Keep writers stopped on both trees throughout.** Live scans run sequentially,
not as a simultaneous filesystem snapshot. Catalog comparison says only what was
recorded, not what is still present. Neither command changes or accepts baselines.

See [the drive workflow](docs/replica-workflow.md) for a staged working-copy/replica test and
baseline procedure, plus remaining prerequisites for the definitive whole-drive run.

## Scope and safety

- Only regular-file contents and their relative paths are cataloged. Directories
  are traversed, but empty directories and filesystem metadata are not recorded.
- Symlinks (including directory symlinks), sockets, devices, and FIFOs cause an
  error. They are not silently ignored or followed. Hard-linked paths are each
  physically read and checked; link topology is not preserved.
- Everything below the chosen root is included except the root's reserved
  **`.keeper/`** directory. There are no implicit `.git`, hidden-file, or mount-point
  exclusions. Nested `.keeper` directories are ordinary data. Don't store user
  data in the root's `.keeper/`.
- Stop dataset writers while running Keeper. File identity, size, mtime and ctime
  are checked before/after hashing, and directory changes during traversal are
  checked. This catches common races, **not** every change after a file was checked.
  Keeper does not provide a filesystem snapshot or defend against an attacker
  concurrently replacing ancestor directories. Read-only snapshots are preferable.
- Plain `status`, `preflight`, `compare` (both modes), and `catalog check` do not write
  Keeper metadata or dataset contents. `status --resume` writes disposable work
  records under `.keeper/`, never the baseline. The filesystem may update access times as
  a side effect of reading. Standalone preflight does not open regular files;
  it does not certify their readability or test lock/fsync/publication support.
- Publication requires filesystem support for advisory locks, file/directory
  `fsync`, and Linux `renameat2(RENAME_NOREPLACE)`. Unsupported operations fail;
  Keeper does not fall back to an unsafe overwrite. Crash durability ultimately
  depends on the filesystem, mount options, and hardware honoring these operations.
- Failed initialization can leave `.keeper/LOCK`; abrupt termination may leave
  `.keeper/.init-*` staging directories. They are not authoritative. Retrying init
  is safe if there is no `catalog`. Do not delete an existing catalog to accept
  changes. A published catalog is never overwritten by `init`.
- Inventories (including stat metadata), catalogs, and preflight issues are held
  in RAM. Traversal/hashing is synchronous. Live comparison holds information for
  both trees. Checkpoint records and their text also consume RAM. Plain status
  and live comparison always pay full-data read cost; opt-in resumable sessions
  reuse earlier per-path observations, not fresh evidence. An interrupted file
  must be reread. Per-file checkpoint fsync can be costly for small-file corpora.
  See [measurements and qualification](docs/qualification.md).

Copy `.keeper/catalog/` off-device. Its checksums detect accidental damage to tables,
not malicious coordinated edits of tables and manifest. A catalog beside its data
is not an independent backup or a cryptographic trust anchor.

## Documentation

- [Design principles, metadata longevity, and migration policy](docs/principles.md)
- [Catalog format and manual inspection](docs/catalog-v1.md)
- [Checkpointed sessions, cancellation, and recovery](docs/resume.md)
- [Filesystem qualification and resource measurements](docs/qualification.md)
- [Replica/drive workflow](docs/replica-workflow.md)
- [Next milestones and unresolved decisions](docs/roadmap.md)
- [Long-form design](docs/design.md), adapted from the original planning notes.
  This is the architectural direction, not a claim that all proposed features exist.

The roadmap captures the full delivery arc and whole-drive readiness gates. The
principles document records compatibility commitments separately from currently
implemented capabilities; migration tooling, for example, is not implemented yet.
