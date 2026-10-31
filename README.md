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

Reports summarize implicated directories; add `--long` for individual files.
Metadata-only checks cannot detect equal-sized content changes. Likely junk is
classified, not excluded, and still counts as a difference. No comparison copies,
deletes, or changes your files.

To establish reusable baselines for two drives, initialize each independently,
then compare their catalogs. Follow the [replica workflow](docs/replica-workflow.md)
for the full procedure, including preparation and later integrity checks.

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
