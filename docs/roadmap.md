# Milestone progression and readiness gates

The immediate product goal is to establish reusable checksums for a large dataset
and its independent replicas, prove path/content agreement, then detect later drift.
A serious daily-use tool is the aim; the first releases are deliberately conservative
foundations, not disposable indexing experiments.

## Delivery arc

| Milestone | State | Outcome / exit condition |
| --- | --- | --- |
| Genesis and physical status | Done: `85debcc` | Inspectable canonical format, no-overwrite init, catalog self-check, read-only full-byte drift detection |
| Replica comparison | Done: `43766e6` | Explicit recorded vs live scope, preflight, stderr progress, failure tests, format-1 compatibility fixture |
| Resumable large-drive operation | Implemented; qualification gate open | Opt-in checkpointed init/status and cancellation; actual target-filesystem qualification and representative measurements still required |
| Explicit acceptance | Design draft | Selectively commit intent revisions without rereading inherited files; safely publish and retain history |
| Semantic validators | Planned | Object/validator-version evidence, direct subprocess orchestration, separate validity from byte identity |
| Rolling maintenance | Planned | Age, failure, budget, and file-count selection with durable progress and honest per-replica physical evidence |
| Richer backup comparison | Planned | Object-presence checks across renamed/moved trees, explicitly separate from exact replica comparison |
| Performance tuning | After correctness and measurements | Storage-aware I/O/hash/validator concurrency and traversal/checkpoint tuning |
| Optional extensions | Deferred | Metadata history, chunk hashes, repair from known copies, scheduling integrations, signing, richer reports |

The original [design stages](design.md#implementation-stages) remain useful scope
breakdowns, not the current release order. Exact comparison and resumability were
promoted ahead of acceptance/validators because establishing trustworthy drive
baselines is the immediate need. Rename inference and incremental scans are useful,
but must not delay or weaken that physical-verification path. Performance measurement
starts early; optimization and additional concurrency follow evidence.

Each milestone must preserve [the design principles](principles.md), especially
baseline immutability, independently checkable metadata, migration without unnecessary
rehashing, and the distinction between object evidence and checking a physical copy.

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
- `compare --metadata-only LEFT RIGHT`: selected current paths/sizes without
  hashing, directory summaries by default, and diagnostic incomplete inventories.
- Explicit root [`.keeperignore` policies](ignore.md), recorded in new scoped
  catalogs and pinned by checkpoints; mismatched comparison scopes return 2.
- Progress and read throughput on stderr, including inside large files.
- Deterministic tests for mutation during read, namespace changes since preflight,
  failed/short/long/interrupted reads, and process death before/after publication.
- Golden format-1 fixture captured from the foundation release, confirming existing
  catalogs remain usable without reindexing and new serialization is unchanged.

Other tests cover byte-level filename escaping, duplicate objects and hard-linked
copies, baseline immutability, no-replace publication, writer exclusion, failed-init
retry, malformed catalogs, comparison scope, and refusal of symlinks/special objects.

## Implemented: resumable operations; whole-drive qualification still open

`init --resume` and `status --resume` now support synced per-path checkpoints,
SIGINT/SIGTERM cancellation, and session/root/baseline identity validation. Complete
corrupt records fail closed; an interrupted trailing append is safely discarded.
Changed file fingerprints are rehashed, including ctime changes with restored mtime.
Each duplicate physical path has independent evidence. Tests exercise clean
cancellation, forced process death, stale records, wrong roots/baselines, writer
exclusion and unchanged format-1 baseline behavior.

Work timestamps are **disposable session observations**, not durable verification
history. Resumed results explicitly report earlier reads and never claim a fresh
scrub. Plain `status` remains full-byte and read-only. No acceptance, live-compare
resume, partial-file hashing, or implicit inclusion/exclusion policy was added.
Explicit exclusions are now supported through `.keeperignore`, with recorded scope.
See [resume semantics](resume.md) and [local measurements / qualification procedure](qualification.md).

The following original milestone requirements remain the whole-drive checklist;
implementation and local smoke measurements do not fulfill actual target-drive
qualification. Before the recommended definitive whole-drive baseline:

- Implemented: clean cancellation and resumable initialization/verification with
  validated durable checkpoints. Fresh physical passes still bypass all work state.
- Implemented: scan/session identity, root identity, checkpoint version, and conditions
  for invalidating/rechecking completed work on resume. File contents can change
  without size/mtime changes; never describe reused metadata as a fresh full scrub.
- Implemented: **per-path physical observations**, with disposable work timestamps,
  not durable history. Reading one duplicate never checks another physical path.
- Test actual target filesystems on disposable copies: locks, sync, no-replace
  publication and failure behavior. Existing process-kill tests do not establish
  actual power-loss durability or target-device compatibility.
- Benchmark traversal, metadata inventory memory, and hashing on representative
  samples. Current inventories/catalogs are in RAM, including both trees during
  live comparison. Avoid worker tuning before measurements.
- Implemented: explicit root `.keeperignore` scope for unsupported system objects,
  without implicit exclusions, dangerous ignore defaults, or quiet partial
  success. Target-drive qualification must still verify the chosen coverage.

Keep `status` as an explicit full-byte operation until an incremental `status`
versus physical `scrub` distinction is documented and tested. Preserve format-1
read compatibility; schema-only upgrades must not force a data rehash.

### Whole-drive readiness gate

Before recommending the definitive baseline run, demonstrate:

1. Safe cancellation/resume and rejection or rechecking of stale/wrong-root work.
   Do not infer fresh integrity solely from matching size/mtime metadata.
2. Successful disposable-sample tests on each actual filesystem/mount: metadata
   encoding, regular-file reads, locking, synchronization, no-replace publication,
   deliberate drift, and interruption behavior.
3. Representative large-file/small-file time and memory measurements, with bounded
   work/resource expectations. Current smoke tests do not establish terabyte-scale
   performance or qualification of arbitrary filesystems.
4. Explicit corpus scope and unsupported-object policy, with no concealed omissions.
5. Independently built catalogs that compare as intended, off-device catalog
   preservation, and repeatable physical drift detection without modifying a baseline.

Semantic validation is valuable but not required just to establish byte agreement.
Agreement still does not prove the original bytes were healthy. Reusing recorded
catalogs avoids an unnecessary immediate second full read; future scrubs deliberately
pay that read cost to obtain new evidence. No elapsed-time estimate substitutes for
these readiness checks.

## Then: explicit acceptance

The [tree intent and staggered verification draft](acceptance.md) specifies the
minimal model and proposes a file layout and publication/recovery protocol.
Immutable, locally numbered **intent revisions** record complete expectations;
independent local observations record physical measurements. Verification compares
the two. Accept only reviewed deltas without rereading inherited files. No unknown
expected hashes, branching, distributed coordination or content-addressed revision
IDs initially. Exact new text schemas and implementation remain pending.

Do not implement baseline updates as an overwrite of `paths.tsv`.

- Specify review/accept semantics for additions, deletions, changes and renames.
  Support the recurring growing-dataset workflow: review each replica against
  its previous baseline, synchronize using an external tool and explicit authority
  policy, verify current-tree agreement, then intentionally commit reviewed changes.
  Accepting expected new files must not implicitly accept changed bytes in old files.
  Agreement between replicas is evidence of equality, not authorization to rebaseline.
- Implement atomic intent-reference publication, recovery, locking/read consistency,
  and revision/format compatibility, including lossless metadata-only migration.
  Preserve existing format-1/2 `generation` fields; the new intent format is separate.
- Revalidate selected new/replacement bytes before acceptance; never promote a
  stale report. Inherited expectations need no new physical read merely to create
  an intent revision; carrying them forward must not refresh their read evidence.
- Preserve the existing baseline on every rejected or failed acceptance.

## Later: semantic validation

- Separate known bytes from structural validity. Invoke validators directly,
  without a shell; bound execution time and capture diagnostic output safely.
- Add validation records keyed by full object hash plus validator/version identity.
- Cover representative image, RAW, video, archive, audio, and document formats
  through external tools, not custom format decoders.

## Then: rolling maintenance

- Add `--older-than`, `--never`, `--failed`, `--budget`, and file-count selection
  only after their evidence and resume semantics are defined.
- Checkpoint useful completed work periodically and on clean cancellation. Bound
  repeated work after failure without making work state the accepted baseline.
- Prioritize never-checked, failed, or oldest evidence conservatively. State the
  verification interval; a multi-session pass is not a simultaneous snapshot.

## Later: richer backup comparison

- Add object-presence backup comparison separately from path-exact replica checks.
  Do not conflate path identity, content identity and physical replica trust.
  Repair, copying, and write-through import remain separate work.

## Measured tuning and optional extensions

Benchmark HDD, SATA SSD, and NVMe behavior as relevant, including many small files,
large files, validators, and checkpoint costs. Optimize storage passes/access patterns
before hash arithmetic; do not assume more parallel readers help an HDD.

Metadata-only catalog history, chunk hashes, repair, signing, scheduling integrations,
and richer reports remain optional follow-on work, not conditions for initial exact
replica checks. No repair/copying feature should arrive implicitly through comparison
or acceptance. Source trust and destructive actions need their own reviewed policy.

The wider [design](design.md) describes these ambitions; [the principles](principles.md)
record the durable commitments. Storage-policy wishes for trust ranking and recovery
do not turn these milestones into a backup engine. See [the drive workflow](replica-workflow.md)
for staged testing.
