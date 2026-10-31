# Design principles and compatibility commitments

These are constraints on Keeper's development, not a claim that every planned
feature exists. See [the roadmap](roadmap.md) for implementation status and
[catalog format 1](catalog-v1.md) for the format actually written today.

## 1. An integrity catalog, not a storage engine

Keeper answers whether files still match accepted bytes and, eventually, whether
those bytes pass format-specific validation. It does not store file contents or
replace synchronization, backup retention, filesystem recovery, or version control.
Keep original assets in ordinary application-neutral files. Keeper and any current
editor, validator, or backup program should remain replaceable clients of that data.

Rust was chosen both as a learning project and for practical systems-tooling
reasons; see [the README rationale](../README.md#why-rust). Learning is not a reason
to add architectural complexity or compromise integrity guarantees.

## 2. Observe, compare, and accept are separate actions

A catalog is the baseline to compare against, not simply the latest scan result.
Routine verification must never silently replace expected bytes with observed
bytes. Initial catalog creation records present bytes; it cannot certify that they
were good beforehand. Future acceptance must be explicit, reviewable, and atomic.

A mismatch is a fact, not a diagnosis. It may reflect an intentional edit, corruption,
a partial write, or another cause. Heuristics can inform investigation, but must not
invent history. Likewise, identical hashes establish agreement, not provenance or
semantic validity. A successful copy, readable rescue image, matching file counts,
or a healthy drive status does not replace content verification.

## 3. Path, content, and physical copy have different identities

- A relative path identifies an expected location within a chosen dataset.
- A full BLAKE3 object ID identifies exact bytes. Short hashes are display-only.
- A physical replica is a particular copy on a particular dataset/device.

Rename or duplication does not change content identity. Successful semantic
validation can be associated with an object and validator/version, independent of
path. But hashing one duplicate does **not** verify another physical copy. Future
scrub history and resumability must preserve that distinction; a shared object-level
"last verified" timestamp cannot stand in for checking every replica.

Exact replica comparison is path-sensitive. A later object-presence backup check
may tolerate moves, but must be a separately stated comparison mode. Hard-link
topology and filesystem metadata are not implied by identical content hashes.

## 4. Plain text is authoritative; caches are disposable

The complete authoritative catalog must be portable, human-inspectable, and usable
without Keeper. Paths must remain reversible even for non-UTF-8 Unix names. Define
canonical encodings, sorting, reference invariants, and checksums explicitly.

Opaque indexes, work queues, and acceleration databases may exist only as
reconstructable state. Losing cache can cost time, not destroy accepted knowledge.
Long-running work may checkpoint observations, but that must not silently turn a
cache into an accepted baseline or an unverified claim of physical integrity.

Catalog self-checks detect accidental damage, not coordinated malicious changes to
tables and manifest. Keep independent, preferably versioned catalog copies; checksums
are not signatures, a trust anchor, or a substitute for backing up the actual data.

## 5. Metadata evolution should preserve expensive knowledge

**A schema-only upgrade must not require rehashing the dataset.** Preserve full
content IDs, byte sizes, reversible path mappings, and any existing authoritative
evidence. Recomputing checksums of rewritten metadata is expected; rereading all
file contents just to rearrange metadata is not.

Keep these version concepts separate:

| Version/identity | Meaning |
| --- | --- |
| Application version | Keeper implementation release, currently a Rust v2 rewrite |
| Catalog format version | Meaning and encoding of authoritative metadata; currently 1 |
| Catalog generation | An accepted state within a catalog, not a schema version; currently genesis only |
| Cache/checkpoint version and session identity | Reconstructable per-path session work state; work format 1, see [resume.md](resume.md) |
| Validator identity/version | Which semantic check produced which evidence; not implemented yet |

Compatibility policy for future work:

1. Keep reading catalog versions already written, or provide a validated, lossless
   migration path. Application upgrades alone must not invalidate a catalog.
2. Bump the catalog format for incompatible schema/semantic changes; never silently
   reinterpret an old version or repurpose an existing column.
3. Validate the input catalog before migration. Preserve expected hashes and paths;
   do not "upgrade" by scanning current files and accepting whatever is there.
4. Construct and self-check migrated metadata separately. Preserve the original
   and publish safely; errors must not damage the only baseline. The concrete
   multi-generation/migration protocol still needs design and implementation.
5. Do not discard authoritative evidence or invent freshness timestamps. If a
   conversion/export would lose information, it is not a lossless migration and
   must not be presented as one.
6. Reject unknown versions explicitly rather than guessing a best-effort parse.
7. Keep golden fixtures and compatibility tests across releases. Format 1 already
   has a fixture captured from foundation commit `85debcc`.

This is not a promise that file contents never need rereading. A fresh physical
scrub, a new hash algorithm, new semantic validation, or unavailable/insufficient
evidence legitimately requires reads. It is a promise not to throw away reusable
hash knowledge merely because the program or metadata layout changes.

**Current limit:** no migration command exists. The first two milestones write the
same format-1 tables, so no migration or reindexing is needed between them.

## 6. Fail closed and make the scope visible

No success report may disguise unreadable files, unsupported objects, invalid
catalogs, partial scans, or detected concurrent mutations. Keep operational errors
distinct from completed comparisons that found differences. Report escaped paths
and facts; keep progress on stderr and results on stdout.

State exactly what was checked: recorded catalogs, filesystem metadata, current
bytes, semantic validity, or some explicitly selected subset. If future inclusion
rules exclude files, report that scope rather than claiming the entire drive
matches. Whole-drive filesystem fidelity is not implied by matching regular files.

Stop writers or use read-only snapshots. Metadata race detection is helpful, not
a snapshot or a defense against hostile concurrent directory replacement. Sequential
scans of two trees are not proof of agreement at one simultaneous instant.

## 7. Atomicity before convenience

Never update authoritative tables piecemeal in place. Stage a complete generation,
validate it, synchronize it appropriately, and publish atomically under writer
exclusion. Never silently weaken guarantees when a filesystem lacks required
operations. Test failure paths, interruption, and actual target filesystems.

Genesis no-replace publication and opt-in resumable checkpoints are implemented.
Safe replacement of generations and migration publication are future work. Process-kill
success is not proof of power-loss durability; filesystem and hardware guarantees
still matter.

## 8. Replica authority is an explicit external policy

Keeper detects agreement and drift; it cannot infer which differing copy should
win. Device speed, a familiar mount name, file timestamps, or a majority of matching
replicas are not by themselves evidence of authority.

A useful external workflow is to commit and verify imports on designated
authoritative storage first, then replicate to a replaceable working copy. Do not
promote unverified working-copy bytes backward or blindly propagate deletions and
replacements. Retain originals until independent verification and backup policy
permit their removal. These are operational policies, not current Keeper mutation
features; future import, copying, or repair tools must be explicitly scoped.

Roles belong to datasets/policies, not a permanently privileged SSD/HDD model or
mount path. Confirm the intended device/corpus when operating on storage. Distinct
partitions on one device do not provide independent failure domains; irreplaceable
data needs a separate-device/off-site, versioned backup and tested recovery.

## 9. Useful work on large datasets, without unnecessary rereads

Correctness comes first, then bounded/resumable operations, then measured tuning.
Prefer a straightforward synchronous pipeline and controlled worker pools over
async or a daemon without demonstrated need. Storage access patterns, small-file
metadata overhead, and avoiding extra full passes usually matter more than hash
arithmetic. HDD and SSD concurrency need different measurements, not assumptions.

Reuse independently established catalogs for recorded comparison rather than
immediately hashing both drives again without a reason. Never equate that saved
work with a fresh physical check. Resumed sessions must state when/where evidence
was obtained and invalidate unsuitable work; size/mtime alone cannot prove absence
of silent corruption. Future age/budget scheduling should make useful bounded
maintenance possible without weakening the meaning of a completed scrub.

Delegate semantic decoding to external validators invoked directly without a shell.
Record validator/version and distinguish PASS, FAIL, and execution errors. Keep
whole-file hashing simple initially; defer repair, chunking, snapshots of catalog
history, daemon operation, and other machinery until a concrete need justifies it.
