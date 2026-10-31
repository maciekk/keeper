# Keeper

A Git-like integrity catalog for ordinary filesystem data. Keeper records which
BLAKE3 content object belongs at each path. Its authoritative metadata is plain
text, not an opaque database. It does not store file contents or replace backups.

This is the Rust rewrite of `maciekk/keeper`. The unused Go experiment is preserved
in Git history (last Go commit: `5d021b2e818bda9e260432b67ac323a22844064e`).

## Current milestone: a conservative foundation

**Early development, not yet a production maintenance tool.** Test on a small copy
first. Do not use the existence of a Keeper catalog as permission to delete originals
or backups. No commands modify dataset files, accept changed content, copy, or repair.

Implemented:

- `init`: hash regular files and publish a self-checked genesis catalog.
- `catalog check`: verify catalog checksums, canonical encoding, and consistency.
- `status`: physically reread **every regular file**, reporting added, missing,
  and changed paths. No metadata shortcut and no baseline updates.
- Full BLAKE3 IDs, reversible Unix filename encoding, deduplicated object records.
- Exclusive writer locking, synced staging, atomic no-replace publication.
- Fail closed on read errors, detected concurrent mutations, unsupported filesystem
  objects, and invalid catalogs. No partial-success status report.

Deferred: explicit acceptance, incremental scans/cache, rename inference, semantic
validators, progress UI, resumability, verification timestamps, backup comparison,
and performance tuning. Renames currently appear as a missing path plus an added
path. This milestone intentionally combines Stage 1 with a minimal full-byte
verification path so the catalog can be exercised end to end.

## Build and try

Requires Linux and a recent stable Rust toolchain (edition 2024). Run from this
checkout:

```sh
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings

# Use an existing dataset directory, not the Keeper source checkout.
./target/release/keeper init /path/to/small-test-dataset
./target/release/keeper catalog check /path/to/small-test-dataset
./target/release/keeper status /path/to/small-test-dataset
```

The dataset root defaults to the current directory. Keeper does not search parent
directories for a catalog. Roots may be explicitly chosen through a symlink; the
resolved dataset root is used throughout.

Exit codes: **0** success/no differences; **1** completed status with differences;
**2** operational error, unsupported object, or invalid catalog. CLI usage errors
also return 2. File paths in catalog rows and status reports use the same escaped
ASCII notation, so control characters cannot forge additional report lines.

`init` records the bytes currently present. **It does not establish that those bytes
are good or format-valid.** Review/validate your sources independently. A hash match
means unchanged bytes, not successful JPEG decoding or a valid archive.

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
- `status` and `catalog check` do not write Keeper metadata or dataset contents.
  The filesystem may update access times as a side effect of reading.
- Publication requires filesystem support for advisory locks, file/directory
  `fsync`, and Linux `renameat2(RENAME_NOREPLACE)`. Unsupported operations fail;
  Keeper does not fall back to an unsafe overwrite. Crash durability ultimately
  depends on the filesystem, mount options, and hardware honoring these operations.
- Failed initialization can leave `.keeper/LOCK`; abrupt termination may leave
  `.keeper/.init-*` staging directories. They are not authoritative. Retrying init
  is safe if there is no `catalog`. Do not delete an existing catalog to accept
  changes. A published catalog is never overwritten by `init`.
- Scans/catalogs are currently held in RAM, traversal is synchronous, and there is
  no resume or progress display. Expect full-data read cost on every status run.

Copy `.keeper/catalog/` off-device. Its checksums detect accidental damage to tables,
not malicious coordinated edits of tables and manifest. A catalog beside its data
is not an independent backup or a cryptographic trust anchor.

## Documentation

- [Catalog format and manual inspection](docs/catalog-v1.md)
- [Next milestones and unresolved decisions](docs/roadmap.md)
- [Original design](docs/design.md), imported from the Obsidian note. This is the
  long-term direction, not a claim that all described functionality exists.
