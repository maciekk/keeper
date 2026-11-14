# Intent formats 1/2, selective acceptance and recovery

Implemented: immutable complete revisions, metadata-only legacy migration/recovery,
exact-parent review plans, and delta-only acceptance. Durable observation logs and
selected/resumed verification passes from [the design](acceptance.md) are **not yet
implemented**. Current `status` and checkpoint semantics remain unchanged.

## Advanced saved-plan workflow

For routine use, prefer [short acceptance commands and snapshots](acceptance-cli.md):
`add`, `rm`, `update`, `mv`, `snap` and `log`, with `--root`, automatic migration
and one confirmation. The explicit workflow below remains available for advanced
review and recovery; it is not a prerequisite for everyday acceptance.

Stop all dataset writers (including during review/application), or use a read-only
snapshot. Start on disposable copies while filesystem qualification is pending.

```sh
# One-time, explicit, metadata-only transition. No data rehash:
keeper catalog migrate /data/photos

# Paths are individual, normalized paths relative to ROOT, not the shell's cwd.
# Plans belong outside the dataset so they do not appear as untracked data.
keeper accept plan /data/photos ~/review.plan --add new.jpg --add another.jpg
keeper accept show ~/review.plan
keeper accept apply /data/photos ~/review.plan

# Replacement/deletion require separate, explicit authorization:
keeper accept plan /data/photos ~/next.plan --replace edited.jpg --remove deleted.jpg
keeper accept show ~/next.plan
keeper accept apply /data/photos ~/next.plan

keeper catalog check /data/photos
keeper status /data/photos
keeper catalog export /data/photos ~/catalogs/photos-R3
```

Repeat each option to select additional files. There are no globs, recursive
additions, implicit deletions or automatic rename inference. A rename is an explicit
`--remove old --add new` pair. Removal requires actual absence: a directory,
dangling symlink, unreadability or other error is not absence. Missing ancestor
directories imply absence; non-directory/symlink ancestors fail closed.

Planning reads only selected additions/replacements, records exact hashes/sizes,
and checks selected removals. Applying the plan is explicit authorization of all its
actions: it rereads selected additions/replacements, requires those approved hashes
and sizes, rechecks removals and selected physical identities, and preserves the
exact exclusion policy. A stale parent or changed selected bytes rejects the entire
batch. There is no automatic rebase or partial publication. Generate/review a new
plan after any rejection; never edit checksums to suppress a stale-plan error.

Unselected expectations are inherited without opening those files or traversing
unrelated directories. Accepting only additions cannot authorize changed or missing
old files. A revision is **intent, not a verification timestamp or snapshot**.
Equal metadata cannot exclude all concurrent edits, especially on NTFS; keep writers
stopped. Acceptance never copies, deletes or changes dataset contents.

Exit codes: 0 success, 2 rejected/error/cancelled. `status` still returns 1 for
differences. Resumable status uses disposable work, not durable observation history;
changing intent contents invalidates an old checkpoint. Cross-revision selected
passes are not implemented.

## Storage and canonical encoding

```text
.keeper/
  LOCK
  legacy-catalog/                 # unchanged original format-1/2 catalog
  intent/
    CURRENT
    revisions/
      1/{MANIFEST,paths.tsv,objects.tsv[,scope.ignore]}
      2/{MANIFEST,paths.tsv,objects.tsv[,scope.ignore]}
    .stage-*/                    # unpublished temporary revision data
    .current-*                   # temporary reference files
```

New `init` still creates legacy catalog format 1/2. Existing catalogs remain usable
without migration. Confirmed short-command acceptance or a named snapshot migrates
automatically; `catalog migrate` remains available explicitly for advanced use/recovery. Do not use older Keeper binaries on migrated roots.
`init` on a migrated root validates/summarizes the active intent, never recreates a
legacy baseline. If `intent/` exists but is incomplete/corrupt, readers refuse it
rather than falling back to the retained legacy catalog.

Revision tables and optional exact `scope.ignore` use the canonical escaping,
sorting, reference and path invariants of [catalogs](catalog-v1.md) and
[scoped catalogs](catalog-v2.md). No extra revision members are permitted. Legacy
`generation` fields are unchanged; the new manifest is a separate schema:

```text
keeper-intent-version<TAB>1
revision<TAB>2
parent<TAB>1
parent-manifest<TAB>b3:...parent manifest checksum...
recorded-unix-ns<TAB>1790000000000000000
file<TAB>objects.tsv<TAB>b3:...64 lowercase hex digits...
file<TAB>paths.tsv<TAB>b3:...64 lowercase hex digits...
file<TAB>scope.ignore<TAB>b3:...64 lowercase hex digits...
```

The last row exists only for scoped revisions. All rows end in LF; integers are
canonical unsigned decimal. Revision numbers are positive u64. R1 has parent 0;
later revisions have a positive, smaller parent number. `parent-manifest` is `-`
for R1 and the full parent manifest checksum otherwise, binding historical manifests
into the validated chain. Time is a canonical u128
Unix nanosecond wall-clock value, not read evidence. Table checksums are BLAKE3 of
the exact corresponding bytes. A revision's manifest checksum is BLAKE3 of its
complete canonical manifest bytes; it detects accidental damage, not authorization.

`CURRENT` is a regular file containing exactly one LF-terminated row:

```text
keeper-current-1<TAB>2<TAB>b3:...manifest checksum...
```

Readers pin it once, validate the referenced numeric child and checksum, and
validate the parent chain. They never recover by picking the highest number.
Symlink directories/members, nonregular members, unsafe references, missing files
and bad checksums fail closed. Local revision numbers are not globally unique;
retain source context and checksum with reports. Root-based reports include both.

### Named snapshots: intent manifest version 2

Named snapshots use intent manifest version **2** (separate from scoped legacy
catalog version 2). They retain exactly the same tables, optional policy and
publication protocol. Relative to the intent version-1 manifest above:

- The header is `keeper-intent-version<TAB>2`.
- Insert `message<TAB>ESCAPED_UTF8_MESSAGE` immediately after `recorded-unix-ns`.
  Messages use canonical Keeper byte escaping and must decode as UTF-8.
- All other rows, checksums and parent-chain rules remain unchanged. The manifest
  checksum covers the message too. Readers support mixed version-1/version-2 chains.

A `snap` with no changes, or `snap --mark`, still publishes a complete new named
revision; it is a historical stopping point, not new physical-read evidence.
Unnamed acceptances continue to write intent version 1. Exports preserve whichever
manifest version the selected revision uses. `log` displays history newest first,
with UTC record times and escaped messages. These wall-clock times are publication
records, not per-file read times. Existing `.work` files are disposable resume state,
not named snapshots. No new durable observation schema is introduced here.

## Plan format 1

Plans are canonical, checksummed ASCII text. Paths use Keeper's byte-reversible
escaping; hashes use full `b3:` IDs. The header pins the canonical root path and
its device/inode, as well as the exact active parent and manifest checksum:

```text
keeper-accept-plan<TAB>1
root<TAB>/canonical/root<TAB>DEVICE<TAB>INODE
parent<TAB>2<TAB>b3:...parent manifest checksum...
add<TAB>new.jpg<TAB>-<TAB>-<TAB>b3:...new hash...<TAB>123
replace<TAB>old.jpg<TAB>b3:...old hash...<TAB>456<TAB>b3:...new hash...<TAB>457
remove<TAB>gone.jpg<TAB>b3:...old hash...<TAB>789<TAB>-<TAB>-
checksum<TAB>b3:...checksum of all preceding bytes, including their final LF...
```

Action rows are sorted by decoded path bytes, not action kind (the example above
is schematic). Fields are kind, path, old hash, old size, new hash, new size. An
absent old/new entry is exactly `-<TAB>-`. Duplicate paths, empty batches, no-op
replacements, mismatched old expectations, policy violations and file/ancestor
conflicts in the complete resulting intent are rejected. The root binding is a
conservative local identity check, not a persistent hardware ID or clone defense.
Copied plans are not authorization to accept on another root. Plans are published
without replacement; use a new filename for each review.

## Publication, recovery, and backup

All migration/plan/accept writers hold `.keeper/LOCK`. Revisions are staged on the
same filesystem, every member is synced, read through the production validator,
and the directory synced before no-replace publication. The revisions directory
is synced before creating/syncing a temporary reference and atomically replacing
`CURRENT`. Finally `intent/` is synced. All prior revisions remain untouched.
Numbers exceed every existing numeric revision directory; abandoned numbers may
leave gaps. Unreferenced directories remain for inspection, never auto-commit.

Failure before reference replacement leaves the previous intent active. A crash
at the switch can leave either complete revision authoritative. **A failure syncing
the directory after the switch is uncertain publication, not guaranteed rollback.**
Inspect `keeper catalog check ROOT` before any retry. A successfully switched plan
will be stale on retry, preventing double acceptance. Missing/corrupt references
fail closed; never manually select the highest revision to silence an error.

Migration stages R1, then renames `catalog/` to `legacy-catalog/` without replacement,
syncs `.keeper/`, and publishes R1's reference. If interrupted before `CURRENT`
exists, repeat **`keeper catalog migrate ROOT`**: it requires exactly one original
catalog location, validates it against any existing R1, rejects unexpected numbered
revisions/conflicts, and finishes only that migration. Original bytes are never
rehash inputs and original metadata is never rewritten. A completed migration is
not repeated. Do not delete an intent store or retained catalog to bypass errors.

`catalog export` on migrated roots exports **one self-contained intent revision**
with its manifest identity, not history or local physical evidence. `compare
--catalogs` reads either legacy exports or intent exports and compares expectations
regardless of local revision numbering. Keep exports labeled with their origin;
their directory names are not globally meaningful identities.

For history backup, separately preserve the **entire `.keeper/intent/`** and the
retained `.keeper/legacy-catalog/` off-device. Stop metadata writers during that
copy. No history pruning/rollback command is supplied. Temporary stages/references
left after a crash are not authoritative; no automatic cleanup is performed.

Tests inject process death at file-write, sync, directory-publication, legacy-rename
and reference-switch boundaries, and exercise stale bytes/plans, deferred paths,
canonical corruption, unsafe members and offline exports. These are process-kill
checks, **not power-loss or target-filesystem qualification**.
