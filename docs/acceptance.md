# Tree intent and staggered verification (design draft)

**Proposed, not implemented.** This draft records the agreed minimal model and a
proposed storage/publication protocol. Exact new text schemas and CLI syntax still
need specification before implementation. Existing commands and format-1/2 files
are unchanged today; plain `status` remains a full-byte, read-only operation.

## 1. Vocabulary and scope

**Tree intent**, usually **intent**, describes which regular files should exist
at which paths, with their full expected BLAKE3 hashes and sizes. It does not
claim that the physical tree currently matches, ever matched simultaneously, or
was recently read. Empty directories and filesystem metadata remain out of scope.

An **intent revision** is an immutable, complete version of those expectations.
The term replaces “generation” in this design, not existing on-disk format fields.

An **observation** records a physical operation, its interval and its result on a
particular local copy. **Verification** compares observations with intent. It is
a computation, not a boolean stored in an intent revision.

The initial feature delivers:

- Selective acceptance: commit some changes and defer others.
- Inherited hashes: do not reread old files merely to create a new revision.
- Separate physical checking: read old files later, potentially across sessions.
- Preserved intent history and local physical observations.

Explicitly excluded initially: branching/merging, distributed coordination,
content-addressed revision IDs, unknown expected hashes, lazy initial enrollment,
automatic history deletion, rollback commands and a general temporal-query engine.
Initial creation still hashes the complete selected dataset before publication.

## 2. Two histories

### Intent history: decisions

Revisions are numbered locally: R1, R2, R3. Each records its parent number,
recorded time, complete path/hash/size mappings and exact exclusion scope. Local
numbers are not globally unique; external reports must retain their originating
catalog context. Independently modified copies must not be merged automatically.
One authoritative writer per history is an operational restriction, not a
cross-device locking guarantee.

Publication makes one revision active. Previous revisions remain immutable and
historical, whether or not they ever received a complete matching verification.
A new revision can describe inherited expectations plus new decisions that never
existed together as a physical snapshot.

Checksums protect metadata integrity; they are not revision identities or proof
of authorization. A separate tree ID is unnecessary initially. Timestamps are
wall-clock records, not trusted clocks; parent order determines revision order.

### Observation history: measurements

Observations are independent of intent publication. A successful hash observation
records a path, full observed hash/size, read interval and local physical binding.
Missing paths and read failures are distinct results; unreadability is not absence.
A verification run also records its target revision and selected scope as context,
but the observed hash is a measurement, not a stored “correct/incorrect” judgment.

Initial durable records need only cover full-file reads and selected presence
checks. General directory-listing event history is not required. An incomplete
traversal must nevertheless prevent claims of complete tree coverage.

History is per physical path: hashing one duplicate does not check another. No
observation from another drive becomes evidence for this drive merely because its
catalog was copied. Use the current resume mechanism's conservative local binding
(canonical root, root device/inode and per-file fingerprints) initially. A changed
root binding requires a new local observation history; old records remain
historical and must not be silently rebound. This may lose continuity after a
remount or root move; it is not a persistent hardware identity or clone defense.

Legacy catalogs supply expectations but no invented per-path read times. Existing
`.work` records remain disposable session checkpoints, not durable history.

### Verification: interpretation

Reports distinguish:

- Observed match during a specified interval.
- Observed mismatch, missing path or failed read.
- No qualifying observation: unknown, not success.

A window can contain both matches and mismatches. An earlier match may inform
scheduling or a continuity assumption but does not count as a new read in a later
window. A later failure must not be hidden behind an earlier successful timestamp.
Matching observations on either side of a gap do not prove continuous agreement.
Different hashes bracket observed change, not its exact time or probability.

Start with latest-outcome and selected-pass coverage reports. Any last-confirmed
summary is derived, with the expected hash, time and physical scope visible.
Neither a single pass nor a resumed pass proves simultaneous conformance without
additional stable-snapshot assumptions. Intent revisions have no permanent
“verified” status.

## 3. Selective acceptance

Acceptance applies explicit actions to an exact parent revision:

- **Add:** authorize a selected previously absent path and full hash/size.
- **Replace:** authorize a specific old hash -> new hash transition.
- **Remove:** authorize dropping a specific expected path, after confirming
  absence. Permission and I/O errors are not evidence of deletion.
- **Rename:** initially an explicitly coupled remove/add; no inferred authority.
- **Defer:** leave intent unchanged. This is not approval or an ignore rule.

An additions-only batch must never accept replacements or removals. Validate
namespace consistency, including file/ancestor conflicts. Initial acceptance
preserves the exact recorded exclusion policy; scope changes are separate work.

A batch is all-or-nothing. If an action fails or is stale, do not silently publish
the rest. The user can explicitly select a smaller batch.

A conservative review protocol is:

1. Observe selected paths and review exact actions, hashes and sizes.
2. Authorize that action list against the parent revision.
3. Under writer exclusion, require the same active parent, freshly hash selected
   additions/replacements and require the approved hashes/sizes. Recheck selected
   absences, physical identities and policy before publication.
4. Construct and publish the new revision; inherit all unselected expectations.

Saved plans also pin a checksum of the parent manifest, detecting accidental
same-number/different-content plans without introducing another identity system.
No automatic rebase onto a newer parent. This can read selected bytes twice;
avoiding that cost is a later measured optimization, not a reason to trust stale
reports. Stop writers or use a read-only snapshot; race checks are not snapshots.

Inherited files need no physical read or mandatory whole-tree traversal during
acceptance. Parent validation, complete metadata serialization and required
selected-path/scope checks still cost work, initially O(catalog size).

Example: R7 expects `old.jpg -> A`, `notes.txt -> B`. Accepting only new
`new.jpg -> C` produces R8 with all three mappings. An unexplained edit to
`notes.txt` remains a difference against B; `old.jpg` is not reread. Later checks
produce observations without changing R8.

## 4. Verification passes across revisions

A pass pins an intent revision and an explicit selected path set. It records its
read interval and distinguishes fresh reads from resumed observations.

Publishing R3 while a pass targets R2 does not retarget that pass or erase its
work. The user may finish it or leave it incomplete and start a pass against R3.
Old measurements remain available, but cannot satisfy a new-read requirement.
Within a resumed pass, fingerprint checks follow the documented resume limitations;
reused reads retain their original times. A fresh pass bypasses such reuse.

Separate completion from agreement:

- Collection coverage: which selected operations finished, failed or remain pending?
- Evaluation: which results match the selected intent?

A collected result may be a mismatch or error; only successful full-file reads
provide hash evidence. Reports must not call missing/error paths successfully
verified. Budget/age scheduling can follow after the basic model works.

## 5. Minimal proposed file layout

```text
.keeper/
├── LOCK                         # existing exclusive writer lock
├── catalog/                     # legacy format-1/2 only, before migration
├── legacy-catalog/              # original catalog retained after migration
├── intent/
│   ├── CURRENT                  # revision number + manifest checksum
│   ├── revisions/
│   │   ├── 1/                   # immutable complete intent
│   │   │   ├── MANIFEST         # version, revision, parent, time, table checksums
│   │   │   ├── paths.tsv
│   │   │   ├── objects.tsv
│   │   │   └── scope.ignore     # only when scoped
│   │   └── 2/
│   └── .stage-*/                # unpublished temporary metadata
└── observations/
    ├── LOCAL                    # version and conservative local-root binding
    └── run-*.log                # append-only checksummed text records
```

Paths/tables retain existing canonical escaping, sorting and reference invariants.
The new manifest needs a new format version; do not reinterpret format 1/2 or
increment their genesis-only `generation` field. `CURRENT` is a small regular text
file, not a symlink, and names only a validated numeric child of `revisions`.
Readers reject symlinks, unsafe references, missing members and bad checksums.

A report/export includes the revision manifest checksum and source context, not
just “R2”. Initially export one self-contained selected intent; do not silently
claim the export includes history or local observations. History backup is a
separate explicit copy of the intent store. Existing format-1/2 export behavior
stays supported for legacy roots.

No global UUID, per-object evidence database, revision-delta chains or secondary
indexes are required. Full revision tables cost extra metadata space; that is an
intentional simplicity tradeoff. Losing disposable acceleration costs time, not
intent or durable observations.

## 6. Publication, readers and failure recovery

All metadata writers use the existing `.keeper/LOCK`. Initially long observation
runs and acceptance serialize; concurrent readers are allowed. No new shared
writer coordination is required.

For a normal intent update:

1. Lock; validate the active intent and approved parent. Complete selected-path
   revalidation before constructing the new revision.
2. Allocate a number greater than existing revision directories; abandoned numbers
   may leave gaps. Record the active revision as parent, not number minus one.
3. Write a staging directory on the same filesystem. Sync every file, read it
   through the real reader, check invariants, then sync the staging directory.
4. Publish the numbered directory with no-replace rename; sync `revisions/`.
5. Write a temporary `CURRENT` containing its number and manifest checksum; sync
   it. Atomically rename it over `CURRENT`, then sync `intent/` before success.

Readers read `CURRENT` once, validate its checksum-bound revision and retain that
revision for the entire operation. Revisions are never deleted initially, so a
concurrent publication cannot invalidate a reader's pinned tables.

Failures before the reference switch leave the previous intent active; a complete
but unreferenced directory is not automatically committed. Never select the
highest directory number as recovery. Readers follow only `CURRENT` and the
validated parent chain for published history. Preserve abandoned metadata for
explicit inspection; cleanup is not part of initial implementation.

A crash around the reference switch may leave old or new `CURRENT`, subject to
filesystem guarantees. Either must reference a complete revision. A failed final
sync is an uncertain publication, not a promise of rollback: report that clearly,
inspect `CURRENT` on retry and never blindly repeat acceptance. A missing/corrupt
reference fails closed rather than guessing a revision. Do not silently fall back
to legacy metadata when an intent store exists.

### Legacy migration

Migration is explicit and metadata-only, under the same lock:

1. Validate `.keeper/catalog/`; refuse conflicting preexisting migration paths.
2. Build and sync a new intent store containing R1 with exactly the old expected
   paths/hashes/scope, no invented physical evidence, and no `CURRENT` yet.
   Sync newly created parent directories as well.
3. Rename the original `catalog/` to `legacy-catalog/` without replacement and sync
   `.keeper/`. Keep those original files unchanged.
4. Publish/sync `CURRENT` for R1 using the normal reference protocol.

Migration is deliberately an offline transition: readers may refuse the root until
it completes. If interrupted, explicit migration recovery validates the retained
original and staged R1 and finishes only that migration; it does not hash live
files or guess a newer intent. A crash before the legacy rename leaves the
original at `catalog/`; after it, the original is at `legacy-catalog/`. If required
files disagree or are missing, refuse and require investigation. Do not use older
Keeper builds on migrated roots: they do not understand the new store. Moving the
legacy directory prevents their ordinary reads from silently reporting a stale
baseline, but cannot prevent every write by obsolete software.

### Observation durability

Each log has a versioned checksummed header identifying its local binding, run,
target revision/checksum and scope. Records are checksummed canonical text with
read intervals, escaped paths, physical fingerprints and hash/absence/error
results. Exact field encoding and scope-list storage are the next schema task.

Append complete results under the writer lock and batch sync, as with existing
checkpoints. Distinguish completed work from synced work: power loss may lose the
unsynced tail. Sync on clean stop/completion before claiming durable coverage.
On recovery discard only an incomplete trailing append; complete corrupt records
fail closed. Record run completion only after the result records are durable;
its absence means coverage must be reconstructed, not assumed complete.

No cross-store transaction is needed. Acceptance publishes intent independently
of whether its reads were also logged. A crash can lose evidence without losing
or changing the committed intent. Never infer a read time from intent publication.

## 7. Implementation order and tests

1. Finalize new manifest/reference schemas. Implement immutable revision reading,
   publication and explicit legacy migration/recovery first, without acceptance.
2. Add selective acceptance, exact-parent plans and delta-only reads.
3. Finalize local observation schemas; add durable logging and basic latest-outcome
   and coverage reports. Keep existing checkpoint semantics until explicitly changed.
4. Add resumed selected passes; later add budget/age scheduling if needed.

Test old format fixtures; lossless metadata-only migration; reader consistency;
no-overwrite revision publication; stale plans; deferred differences; changed
selected bytes; cancellation; and process death at every file/rename/sync boundary.
Include orphan revisions, corrupt/missing references, interrupted migration and
observation tails. Test that new intent does not refresh old evidence and copied
logs cannot claim new-root physical coverage. Existing work is not a substitute
for durable-log tests.

Qualify actual target filesystems separately. Process-kill tests do not prove
power-loss durability. Builds must refuse unsupported locking/publication/sync
operations rather than weakening the protocol.
