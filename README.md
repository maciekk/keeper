# Keeper

Keeper detects changes to files and compares independent copies of a dataset.
It records BLAKE3 hashes and relative paths in an inspectable, plain-text catalog,
then checks files against that baseline. Your files stay ordinary files: Keeper
is not a backup, synchronization, or repair tool.

**Early development—not yet a production maintenance tool.** Start with a small,
disposable copy. Target-drive qualification and large-corpus measurements are
still pending; see the [drive workflow](docs/replica-workflow.md) before a
whole-drive baseline.

## Build

Requires Linux and a recent stable Rust toolchain (edition 2024).

```sh
cargo build --release --locked
./target/release/keeper --help
```

## Record a baseline and check it later

Use a dataset directory, not the Keeper source checkout. Stop writers while
scanning, or use a read-only filesystem snapshot.

```sh
K=./target/release/keeper

"$K" preflight /path/to/data      # Inspect metadata and report scan problems
"$K" init /path/to/data           # Read and hash files; create .keeper/catalog/
"$K" catalog check /path/to/data  # Validate the catalog itself
"$K" status /path/to/data         # Reread files; report added/missing/changed paths
```

`init` never overwrites a baseline. If a catalog already exists, it validates the
catalog and reprints its recorded statistics without rescanning dataset bytes
(also with `--resume`). Invalid or unreadable catalogs produce a specific error;
use `status` to verify current bytes. `status` never accepts changes. A baseline
records current bytes, not proof that they were good or format-valid. Back up
`.keeper/catalog/` off-device as well as keeping independent backups of your data.

For interruptible long scans, use `init --resume` or `status --resume` and repeat
the same command after interruption. Resuming reuses earlier reads in that session;
it is **not a fresh full scrub**. See [resuming and recovery](docs/resume.md).

## Compare two copies

```sh
# Quick check of current paths and sizes; no hashing or catalog needed:
"$K" compare --metadata-only /path/to/left /path/to/right

# Read and hash both trees; no catalog needed:
"$K" compare --live /path/to/left /path/to/right

# Compare previously recorded baselines; does NOT check current file contents:
"$K" compare /path/to/left /path/to/right
```

### Compare offline, without accessing either drive

Export each drive's existing baseline once while it is connected (reads only
catalog metadata, not dataset contents):

```sh
mkdir -p ~/keeper-catalogs
"$K" catalog export /path/to/ssd ~/keeper-catalogs/ssd
"$K" catalog export /path/to/DATA2 ~/keeper-catalogs/DATA2

# Both drives can now be disconnected:
"$K" compare --catalogs ~/keeper-catalogs/ssd ~/keeper-catalogs/DATA2
```

Destinations must not already exist; use a new name for each snapshot. You can
also compare existing local copies of complete `.keeper/catalog/` directories
with `--catalogs`. Exported catalogs describe the recorded baseline, not changes
made since hashing. No source-drive paths are resolved during offline comparison.

Reports summarize implicated directories; add `--long` for individual files.
Metadata-only checks cannot detect equal-sized content changes. Likely junk is
classified, not excluded, and still counts as a difference. No comparison copies,
deletes, or changes your files.

To establish reusable baselines for two drives, initialize each independently,
then compare their catalogs. Follow the [replica workflow](docs/replica-workflow.md)
for the full procedure, including preparation and later integrity checks.

## Live files, baselines, and exported catalogs

Keeper has three distinct sources of information:

- **Live files:** the current contents of a dataset.
- **Default catalog (`ROOT/.keeper/catalog/`):** the baseline recorded by `init`.
  This is saved checksum information, not an automatically refreshed cache.
- **Exported catalog:** an independent copy of that baseline elsewhere. Export
  does not inspect current dataset contents or create a fresh observation of them.

Commands select their inputs explicitly; there is no automatic detection or
fallback from a missing catalog to hashing files:

| Command | Inputs read |
| --- | --- |
| `compare LEFT RIGHT` | Each dataset root's `.keeper/catalog/` only |
| `compare --catalogs LEFT RIGHT` | The two standalone catalog directories only |
| `compare --live LEFT RIGHT` | Current files in both trees, freshly hashed |
| `compare --metadata-only LEFT RIGHT` | Current paths and sizes, not file contents |
| `status ROOT` | Default catalog plus freshly hashed current files |
| `catalog check ROOT` | Default catalog only |
| `catalog export ROOT DEST` | Default catalog; writes a new standalone copy |
| `preflight ROOT` | Current filesystem metadata, not file contents |

The comparison modes are mutually exclusive. Default comparison requires access
only to the catalogs on the drives, not their data files. Export is optional:
use it to avoid accessing the original drives at all. Drives need not be
physically disconnected when comparing local exports.

### Growing datasets and monthly checks

**Rerunning `init` does not refresh a baseline.** An existing catalog is validated
and summarized, not overwritten or extended. `status` reports additions, removals,
and changes but never accepts them. Neither kind of comparison updates catalogs.

For a monthly check, run `status` on each drive and investigate unexpected changes.
Synchronize with your separate backup tool under an explicit source-to-backup
policy, then use `compare --live` if you need to verify current-tree agreement,
including newly added files. Stop writers during checks. Default `compare` still
compares the old recorded baselines and cannot reveal subsequent additions.

**Reviewing and accepting a newer baseline is not implemented yet.** The planned
workflow must let users accept expected additions without automatically accepting
unexpected changes to old files, preserve prior generations, and revalidate bytes
before acceptance. Do not delete catalogs and rerun `init` to silence differences;
that discards the baseline evidence. See the [replica workflow](docs/replica-workflow.md)
and [acceptance roadmap](docs/roadmap.md#then-explicit-acceptance).

## Know the limits

- Only regular-file contents and paths are cataloged—not empty directories or
  filesystem metadata. Symlinks and special files are unsupported.
- Everything under the selected root is included except Keeper configuration and
  explicit root-level [`.keeperignore` exclusions](docs/ignore.md). There are no
  automatic system-folder exclusions. Set the policy before indexing.
- Stop writers throughout a scan, including pauses. Keeper is not a snapshot.
- Exit codes: **0** match/success, **1** differences, **2** error or incomplete check.
  An incomplete metadata report is diagnostic output, not a successful comparison.

Use `keeper help <command>` for focused help, or consult:

- [Usage reference: commands, reports, options, and safety details](docs/usage.md)
- [Drive/replica workflow](docs/replica-workflow.md)
- [Resumable scans and recovery](docs/resume.md)
- [Exclusions and scan scope (`.keeperignore`)](docs/ignore.md)
- [Catalog format and manual inspection](docs/catalog-v1.md), [scoped catalogs](docs/catalog-v2.md)
- [Development and build checks](docs/development.md)
- [Design principles](docs/principles.md), [roadmap](docs/roadmap.md), and
  [filesystem qualification](docs/qualification.md)

The [long-form design](docs/design.md) describes architectural direction, not a
claim that every planned feature exists.
