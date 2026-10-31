# Exercising replica comparison and preparing large-drive baselines

The near-term use case is a working dataset (for example, on an SSD) and an
independent external-drive replica. Device type does not determine authority.
Read-only replica comparison and opt-in checkpointed init/status are implemented;
whole-drive qualification is still pending. Start with disposable copies, not
valuable originals. See [resume semantics](resume.md) before reusing earlier reads.

## 1. Disposable end-to-end test

From the checkout, after `cargo build --release --locked`:

```sh
K="$PWD/target/release/keeper"
T="$(mktemp -d)"
mkdir "$T/left" "$T/right"
printf 'original\n' > "$T/left/a"
printf 'second\n' > "$T/left/b"
cp -a "$T/left/." "$T/right/"

"$K" preflight "$T/left"
"$K" compare --live "$T/left" "$T/right"   # 0: current bytes agree
"$K" init "$T/left"
"$K" init "$T/right"
"$K" compare "$T/left" "$T/right"          # 0: recorded catalogs agree

printf 'modified\n' > "$T/right/a"
rm "$T/right/b"
printf 'new\n' > "$T/right/c"
"$K" compare "$T/left" "$T/right"          # still 0: old records agree
"$K" compare --live "$T/left" "$T/right"   # 1: different, only left, only right
"$K" status "$T/right"                    # 1: changed, missing, added

printf 'Sandbox retained at: %s\n' "$T"
```

Expected difference commands return 1; if adapting this to a script using `set -e`,
handle that explicitly. Exit 2 means an incomplete check/error, never equality.
`status` and comparison leave the catalogs untouched. Do not delete/recreate the
catalog just to silence differences.

## 2. Small representative copy on each actual filesystem

Before whole-drive initialization, place a disposable test directory on each
intended filesystem. Include varied file sizes, many small files, non-ASCII names,
and real copies of your usual photo/music/archive formats. Do not use hard links
to valuable originals: modifying a hard-linked test file would modify the original.

Run preflight, initialization, catalog checks, recorded comparison, fresh comparison,
and intentional modifications on those disposable copies. This exercises advisory
locks, directory fsync and no-replace rename on the actual mounts without baselining
or modifying the valuable corpus. Plain read-only checks do not create `.keeper`;
`init` and opt-in checkpointed status write Keeper metadata. Follow the
[filesystem qualification procedure](qualification.md), including cancellation tests.

Preflight inventories every supported file and byte size using metadata. It lists
all encountered traversal/unsupported-object issues rather than stopping at the
first. It does not open regular files, test their readability, or test filesystem
publication support. An actual hashing run may still fail later. There is no ignore
configuration yet; system folders, symlinks, or unsupported files within a drive
root must not be silently discarded to obtain a reassuring match. Choose the
intended corpus root deliberately.

## 3. Definitive baseline workflow (after target-drive qualification)

Paths in these examples are placeholders, **not assumed mount paths**. Confirm
the intended devices, stable identities, mounts, and corpus roots; decide which tree
is authoritative independently of Keeper. Follow an explicit source-to-replica policy,
not automatic reverse synchronization of unverified changes. See [replica authority
and failure domains](principles.md#8-replica-authority-is-an-explicit-external-policy).

1. Stop writers on both trees, or use appropriate read-only filesystem snapshots.
2. Run `preflight` on both chosen corpus roots and resolve reported issues.
3. Run `init` on each tree (or `init --resume` for a checkpointed session; repeat
   that command after cancellation). Keep writers stopped throughout pauses too.
   Each catalog records that tree's own physical observations; no catalog is copied
   from one tree to make the other appear initialized.
4. Run `catalog check` on both, then `compare LEFT RIGHT`. Matching recorded
   catalogs establish agreement at the recorded paths, assuming stable input
   during initialization. Differences require investigation, not automatic
   acceptance or repair. Hash agreement does not prove format validity.
5. Copy each complete `.keeper/catalog/` to an independently stored, labeled,
   preferably versioned off-device location. Preserve which device/corpus each
   catalog belongs to. A directory of tables alone is not a backup of the data.
6. Later, `status ROOT` physically rereads that tree against its baseline.
   Catalog-only comparison does not replace these physical checks.

Do not add a redundant `compare --live` immediately after independently hashing
both trees unless you intentionally want another full read. Fresh comparison is
useful for ad hoc checks without catalogs; it does not itself persist a baseline.

Progress can be recorded separately from reports:

```sh
keeper status --progress always /chosen/corpus >status.txt 2>progress.log
```

This implementation has progress, deterministic read/mutation failure tests,
checkpointed init/status, cancellation tests, and process-death tests around
publication and checkpoints. It still has no verified target-drive filesystem
compatibility results or representative large-corpus resource measurements.
Those remain prerequisites to our recommended definitive whole-drive run. Local
[smoke measurements](qualification.md) are not target qualification; process-kill
tests are not power-loss tests.

## Metadata longevity

The [compatibility commitments](principles.md#5-metadata-evolution-should-preserve-expensive-knowledge)
are the project-wide policy; the following describes this milestone's actual state.

The three authoritative format-1 files are unchanged by this milestone. A golden
catalog captured from foundation commit `85debcc` is read and compared in tests;
a new catalog over those same bytes must match it byte for byte. No reindexing is
needed to use existing catalogs with these commands.

Full BLAKE3 IDs, sizes, and reversible relative paths are the expensive reusable
knowledge. Future incompatible formats must declare a new version and provide a
validated, lossless migration retaining the original. Migration tooling does not
yet exist. A schema-only migration should operate on metadata, not reread files.
New kinds of evidence (semantic validation, a new hash algorithm, or a fresh
physical check) may genuinely require additional data reads.
