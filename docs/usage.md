# Usage reference

For a first run, see the [README](../README.md). For independently baselining two
drives and checking them later, follow the [replica workflow](replica-workflow.md).

## Commands and roots

| Command | What it checks or records |
| --- | --- |
| `preflight [ROOT]` | Metadata-only inventory: file counts, sizes, unsupported objects, and traversal errors |
| `init [ROOT]` | Physically hash regular files and publish a self-checked initial catalog |
| `catalog check [ROOT]` | Catalog checksums, canonical encoding, and internal consistency; not dataset bytes |
| `status [ROOT]` | Physically reread every regular file against the baseline; report added, missing, and changed paths |
| `compare LEFT RIGHT` | Recorded paths, sizes, and full content IDs in two catalogs |
| `compare --live LEFT RIGHT` | Current paths and contents, freshly read from both trees without using catalogs |
| `compare --metadata-only LEFT RIGHT` | Current regular-file paths and sizes, without opening file contents or using catalogs |
| `help [COMMAND]...` | General or focused help, including nested commands such as `help catalog check` |

`ROOT` defaults to the current directory. Keeper does not search parent directories
for a catalog. Roots may be explicitly chosen through a symlink; scanning uses the
resolved root. Comparison inventory labels retain the supplied path arguments.

`init --resume` and `status --resume` start or continue checkpointed sessions.
They reuse validated earlier per-path observations, **not fresh full scrubs**.
Keep writers stopped through pauses too. See [resume semantics and recovery](resume.md).
Live comparison has no checkpoint support yet.

Preflight runs automatically before hashing. Live comparison preflights both trees
before hashing either. Standalone preflight does not open regular files, certify
their readability, or test lock, sync, or publication support; hashing can still fail.

No commands accept changed content, copy, repair, or modify dataset files. `init`
writes the initial catalog, and resumable operations write disposable work state.
Plain status, preflight, catalog checks, and all comparisons do not write Keeper
metadata. Reading may cause filesystem access-time updates.

## Comparison scope

Comparison is path-sensitive: identical bytes elsewhere do not establish an exact
replica. Renames appear as one path only on the left and another only on the right.
Neither content equality nor a baseline establishes semantic validity, provenance,
or which replica is authoritative.

- **Catalog comparison** says only what was recorded. It does not check whether
  files still exist or still contain those bytes.
- **Live comparison** physically reads both trees sequentially, not as a
  simultaneous snapshot. No catalog is required, created, or updated.
- **Metadata-only comparison** detects path and size differences, not content
  integrity. Equal sizes do not imply equal bytes. Timestamps and empty directories
  are not compared.

Roots must be distinct and non-overlapping after symlink resolution. Mount aliases
and bind mounts are not fully detected; choose genuinely separate datasets. Stop
writers throughout the operation, or use suitable read-only snapshots.

Do not add a redundant live comparison immediately after independently hashing
both trees unless you intentionally want another full read.

## Reports

Comparison reports have dedicated sections, with uppercase headings:

1. Overall assessment, flagging incomplete results.
2. Inventory counts, sizes, and the net delta (right minus left).
3. Directory differences, when present.
4. Individual file differences with `--long`.
5. Final notes and issues, as a dimmed bullet list when colour is enabled.

Inventory rows identify supplied roots in the first column, in left/right argument
order. Root labels remain full-length; numeric columns shift right for long paths.
Paths in inventory and directory tables use normal-intensity ANSI blue (`0;34`) when colour is enabled. Each differing
file is counted once under its immediate parent directory (`.` for root-level
files). The directory path is the first column, followed by the counts.
Directory counts are not recursive totals and do not imply that directory
metadata or empty directories were compared. The table shows left-only,
right-only, different, and `Junk` counts.

Root and directory labels retain their full escaped paths, including on terminals;
numeric columns shift right as needed. Individual file records also retain full
escaped paths.
Paths use reversible ASCII escaping, including non-UTF-8 names; control characters
cannot forge report lines. See [catalog encoding](catalog-v1.md).

With `--long`, file records use `ONLY_LEFT`, `ONLY_RIGHT`, and `DIFFERENT`.
Different files include both sizes and, in content-based modes, full hashes.

### Incomplete inventories

Metadata-only comparison lists issues from both sides and still prints an
explicitly **INCOMPLETE** observed-file comparison, returning exit code 2.
`OBSERVED_ONLY_LEFT` and `OBSERVED_ONLY_RIGHT` mean a path was observed on only
one side, not proven absent on the other. Counts, size deltas, and classifications
cannot establish whole-tree agreement or explain the full delta when paths are
unreadable. Hashing operations refuse incomplete inventories; they do not produce
partial-success integrity reports.

### Likely junk

`Junk` is advisory: likely unimportant housekeeping, not proof of data value or
permission to delete. It may cover more categories in future, but current
recognition uses exact, case-sensitive path components:

- `$RECYCLE.BIN`, `System Volume Information`
- `.Trash`, `.Trash-<numeric UID>`, `.Trashes`
- `.Spotlight-V100`, `.fseventsd`, `.DS_Store`

Detailed file records use `category=OS_HOUSEKEEPING` for these recognized names.
Nothing is excluded and such differences still return exit code 1. Trash may
contain valuable user files; unfamiliar OS files remain unclassified. “All
reported differences” being likely junk does not rule out equal-sized content
changes in metadata-only mode.

## Progress, colour, and exit codes

- `--progress auto|always|never`: default `auto` enables progress only on a
  terminal. Terminal updates replace the current line; one leading blank line
  precedes contiguous phase lines. Hashing reports throughput, percentage, and ETA
  when totals are known, including within large files. ETA and elapsed time each
  use a single rounded figure: seconds below 2 minutes, minutes below 2 hours,
  then hours (`119s`, `115m`, `2h`). For files of at least 256 MiB taking more than a
  second to read, terminal progress appends a compact per-file star bar after the
  overall ETA, without extra per-file statistics. Filled stars are green when
  colour is enabled. The bar clears when the file finishes and does not appear in
  redirected logs or for checkpoint-reused files. ETA is never truncated: other
  fields are shortened first, and the bar is omitted if needed. If the terminal is
  narrower than the ETA itself, the complete ETA takes priority over fitting.
- Forced redirected progress records phase starts/completions without terminal
  controls or repetitive updates.
- `--color auto|always|never`: automatic colour checks stdout and stderr separately;
  `NO_COLOR` disables automatic colour. Explanatory notes and table headers are dim.
- Counts use comma separators through 999,999, then compact M/G/T-style suffixes.
  Byte sizes use B/KiB/MiB/etc. Exact values remain in catalog metadata.

```sh
keeper compare --live --progress always /path/to/left /path/to/right \
  >comparison.txt 2>progress.log
```

Store logs outside the scanned roots.

| Exit | Meaning |
| --- | --- |
| 0 | Success / no differences within the command's stated scope |
| 1 | Completed status or comparison with differences |
| 2 | Operational error, unsupported object, invalid catalog, incomplete metadata comparison, cancellation, or CLI usage error |

Scripts using `set -e` must handle expected difference exits explicitly. Exit 2
never establishes equality.

## Filesystem scope and operational limits

- Only regular-file contents and their relative paths are cataloged. Empty
  directories, filesystem metadata, and hard-link topology are not recorded.
  Hard-linked paths are each physically read and checked.
- Symlinks (including directory symlinks), sockets, devices, and FIFOs are
  unsupported. They are not silently ignored or followed during traversal.
- Everything below the root is included except its reserved `.keeper/` directory.
  There are no implicit `.git`, hidden-file, system-folder, or mount-point
  exclusions. Nested `.keeper` directories are ordinary data. Do not store user
  data in the root's `.keeper/`.
- File identity, size, mtime, and ctime are checked before/after hashing; directory
  changes during traversal are checked. This detects common races, not every
  mutation after a file was checked. Keeper does not provide a snapshot or defend
  against an attacker concurrently replacing ancestor directories.
- Inventories, catalog records, and preflight issues are held in RAM. Traversal
  and hashing are synchronous. Comparison holds information for both trees, and
  checkpoint records and their text also consume RAM. Memory grows with corpus
  and journal size; see [measurements and qualification](qualification.md).
- Plain status and live comparison pay the full-data read cost. Resume reuses
  earlier observations, not fresh evidence; an interrupted file must be reread.
  Per-file checkpoint sync can be costly for small-file corpora.

Initialization requires advisory locks, file/directory `fsync`, and Linux
`renameat2(RENAME_NOREPLACE)`. Keeper does not fall back to unsafe overwrite.
Crash durability depends on filesystem and hardware guarantees. Failed init can
leave `.keeper/LOCK` or `.keeper/.init-*` staging directories; these are not
accepted catalogs. Retry only when no catalog exists; never delete a baseline to
silence differences. See the [publication protocol](catalog-v1.md#genesis-publication-protocol)
and [checkpoint recovery](resume.md#work-state-and-recovery).

Copy each complete `.keeper/catalog/` off-device, clearly labeled by corpus and
device. Checksums detect accidental catalog damage, not coordinated malicious
changes to tables and manifest. A catalog beside its data is neither an independent
backup nor a cryptographic trust anchor.
