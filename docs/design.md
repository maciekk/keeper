# Keeper v2 — Design

## Purpose

Keeper is a filesystem integrity tool.

Its job is to answer:

> Are the files in this dataset still the exact files I previously accepted as good, and are those files structurally valid according to their formats?

Keeper is intentionally **not**:

- a backup system
- a version-control system
- a deduplicating object store
- a replacement for `rsync`, Borg, restic, etc.
- a proprietary database containing the only useful copy of integrity metadata

Keeper should instead act as a **thin orchestrator over durable, understandable primitives**.

The core philosophy is:

> Keeper provides convenience. The underlying integrity data and verification methods should remain understandable and usable without Keeper.

This is strongly inspired by UNIX philosophy and by Git's distinction between path identity and content identity.

---

# Goals

Keeper should provide:

1. byte-for-byte file integrity using cryptographic hashes
2. format-aware semantic validation using external tools
3. detection of:
   - corruption
   - modification
   - addition
   - deletion
   - rename/move
4. an authoritative known-good catalog
5. plain-text, human-readable metadata
6. independence from Keeper itself
7. efficient incremental operation
8. explicit full-data scrubbing when desired
9. interruptible and resumable expensive operations
10. easy verification of backups
11. strong behavior in the presence of corruption
12. clear distinction between authoritative state and disposable acceleration/cache state

---

# Non-goals

Keeper should avoid becoming overly ambitious.

Do not initially implement:

- file version storage
- file synchronization
- backup retention
- chunk-level repair
- deduplication
- compression
- cloud storage
- RAID-like redundancy
- a database engine
- filesystem monitoring daemon
- automatic mutation of known-good state

These may integrate with Keeper later, but they are not its core responsibility.

---

# Core Principles

## 1. The catalog is authoritative

The catalog describes what the dataset **should contain**.

It is not merely a record of the most recent scan.

If a file changes unexpectedly, Keeper must not silently update the catalog.

Example:

```text
catalog:
Photos/foo.RAF -> b3:ABC

filesystem today:
Photos/foo.RAF -> b3:DEF
```

Keeper reports:

```text
MISMATCH Photos/foo.RAF

expected: b3:ABC
observed: b3:DEF
```

It does **not** replace `ABC` with `DEF`.

Accepting changed content must be an explicit operation.

---

## 2. Path identity and content identity are separate

A path identifies a location in the filesystem.

A content hash identifies a sequence of bytes.

These are distinct concepts.

```text
Path
    Photos/2026/foo.RAF

points to

Object
    b3:ABC...
```

The same content may exist at multiple paths.

A path may later contain different content.

This is intentionally Git-like.

---

## 3. Content hashes are object IDs

Use full BLAKE3 hashes as canonical object identifiers.

Example:

```text
b3:3f85...full-256-bit-value...
```

Never depend on abbreviated hashes for authoritative identity.

Short hashes may be used for human-facing display.

---

## 4. Plain text is authoritative

Critical Keeper metadata must be plain text.

A user should be able to:

```bash
vim .keeper/catalog/paths.tsv
grep ...
awk ...
sort ...
comm ...
b3sum ...
```

and understand or verify the dataset without Keeper.

Binary formats, SQLite databases, indexes, mmap structures, etc. may exist only as **reconstructable cache**.

Rule:

> Anything opaque may be cached. Nothing irreplaceable may exist only in opaque form.

---

## 5. One catalog directory, not one catalog file

Keeper manages one logical catalog:

```text
.keeper/
```

The catalog itself may contain several files, split by semantic domain.

The goal is not minimum file count.

The goal is:

> Make the authoritative state small enough conceptually that a human can understand it.

---

# Directory Layout

Initial conceptual layout:

```text
.keeper/
├── catalog/
│   ├── MANIFEST
│   ├── paths.tsv
│   ├── objects.tsv
│   └── validation/
│       ├── images.tsv
│       ├── raw.tsv
│       ├── video.tsv
│       ├── archives.tsv
│       ├── documents.tsv
│       └── databases.tsv
│
└── cache/
    └── ...
```

Possible later additions:

```text
.keeper/
├── catalog/
├── cache/
├── snapshots/
└── reports/
```

But v1 should remain conservative.

---

# Catalog vs Cache

## `catalog/`

Contains the complete authoritative state.

Properties:

- plain text
- portable
- human-readable
- sufficient to understand known-good state
- safe to copy independently
- suitable for Git/versioning
- no implementation-specific dependency required to interpret it

## `cache/`

Contains disposable acceleration state.

Examples:

- parsed indexes
- SQLite indexes
- filesystem traversal caches
- work queues
- validator scheduling state
- temporary scan results

Invariant:

```bash
rm -rf .keeper/cache
```

must never destroy authoritative knowledge.

Keeper should be able to rebuild cache entirely from:

```text
catalog + filesystem
```

---

# Core Data Model

Keeper has three core concepts:

## Path

A location in the filesystem.

Example:

```text
Photos/2026/DSCF1001.RAF
```

## Object

A known sequence of bytes identified by its BLAKE3 hash.

Example:

```text
b3:ABC...
```

## Observation

What Keeper discovered during a particular scan.

Example:

```text
Photos/2026/DSCF1001.RAF currently hashes to b3:DEF...
```

Observations are not automatically authoritative.

---

# `paths.tsv`

`paths.tsv` represents the expected filesystem namespace.

Conceptually:

```text
path    kind    object    size    mtime_ns
```

Example:

```text
Photos/2026/foo.RAF	file	b3:ABC...	42192384	1792200100123456789
Documents/tax.pdf	file	b3:DEF...	823719	1792100000000000000
```

The exact column set should remain minimal.

Important distinction:

- path-specific metadata belongs here
- content-specific metadata does not

Potential path-specific fields:

- relative path
- type/kind
- expected content object
- size
- mtime
- mode
- optional filesystem-specific metadata

Do not add fields merely because they are available.

---

# `objects.tsv`

`objects.tsv` describes known content objects.

Conceptually:

```text
hash    size
```

Example:

```text
b3:ABC...	42192384
b3:DEF...	823719
```

Keep this intentionally boring.

Object-level data should only include properties intrinsic to the bytes.

Potential future fields:

- detected MIME type
- first-seen catalog generation

Avoid turning `objects.tsv` into miscellaneous metadata storage.

---

# Validation Tables

Format validation belongs to objects, not paths.

Example:

```text
catalog/validation/raw.tsv
```

Conceptually:

```text
object    format    validator    validator_version    result    validated_at
```

Example:

```text
b3:ABC...	RAF	libraw	0.22.0	PASS	2026-10-16T21:14:03Z
```

This gives important behavior automatically:

### Rename

```text
old-name.RAF -> ABC
new-name.RAF -> ABC
```

Validation remains valid.

### Duplicate

```text
foo.RAF -> ABC
copy.RAF -> ABC
```

The content only needs one semantic validation record.

### Modification

```text
foo.RAF:
ABC -> DEF
```

Validation of `ABC` does not apply to `DEF`.

The new object has no validation record and therefore requires validation.

---

# Why Not Path + Short Hash?

A compound identifier such as:

```text
Photos/foo.RAF#ABC123
```

was considered.

It is rejected as the authoritative key because:

- rename changes half the identifier
- modification changes the other half
- short-hash collision rules become necessary
- joins remain awkward
- full content identity is already available

Short hashes remain useful only for display.

---

# Rename Detection

Rename detection falls naturally out of content addressing.

Previous catalog:

```text
old-name.RAF -> ABC
```

Observed filesystem:

```text
new-name.RAF -> ABC
```

Keeper may infer:

```text
RENAME
old-name.RAF -> new-name.RAF
```

because the content object is unchanged.

No content metadata needs rewriting.

---

# Corruption Semantics

Suppose:

```text
catalog:
foo.RAF -> ABC
```

but today:

```text
observed:
foo.RAF -> DEF
```

Keeper reports:

```text
CHANGED / CORRUPT / UNKNOWN CAUSE

path:      foo.RAF
expected:  ABC
observed:  DEF
```

Keeper must not guess whether this is:

- intentional editing
- bit rot
- partial write
- application rewrite
- malware
- hardware corruption

It reports facts.

Useful heuristics may be shown separately.

For example:

```text
hash changed
size unchanged
mtime unchanged
```

is highly suspicious for silent corruption.

But it remains a heuristic.

---

# Corruption and Backups

Content addressing makes recovery powerful.

Example:

Primary:

```text
Photos/foo.RAF
expected: ABC
observed: DEF
```

Backup:

```text
Pictures/old-name.RAF -> ABC
```

Keeper can recognize that the backup contains the exact expected object regardless of filename.

Potential future operation:

```bash
keeper repair /mnt/data /mnt/backup
```

could locate the expected object by hash.

Repair should not be part of initial implementation.

---

# Ambiguous Rename + Corruption

Example:

Catalog:

```text
old-name.RAF -> ABC
```

Current filesystem:

```text
new-name.RAF -> DEF
```

Keeper cannot prove whether:

1. `old-name.RAF` was renamed and then corrupted, or
2. `old-name.RAF` disappeared and `new-name.RAF` is unrelated.

Keeper should report ambiguity.

Do not fabricate history.

---

# Hash Algorithm

Use BLAKE3.

Reasons:

- cryptographically strong
- extremely fast
- optimized SIMD implementations
- excellent Rust support
- parallelizable
- hashing performance normally exceeds disk throughput
- standard `b3sum` utility exists

Canonical representation:

```text
b3:<full-hash>
```

Store full hashes everywhere authoritative.

---

# UNIX Independence

Keeper should expose data in forms usable with normal tools.

Examples:

```bash
grep 'foo.RAF' .keeper/catalog/paths.tsv
```

```bash
awk -F '\t' '$3 == "b3:ABC..." {print $1}' \
  .keeper/catalog/paths.tsv
```

```bash
b3sum path/to/file
```

```bash
cut ...
sort ...
comm ...
```

Keeper should ideally provide:

```bash
keeper explain path/to/file
```

showing equivalent manual commands.

Example:

```text
Equivalent manual integrity check:

b3sum path/to/file

Expected:
b3:ABC...
```

For semantic validation it could additionally show:

```text
dcraw_emu ...
ffmpeg ...
7z t ...
qpdf --check ...
```

Keeper should teach the abstraction it provides.

---

# Filename Encoding

UNIX filenames may contain:

- tabs
- newlines
- backslashes
- arbitrary byte sequences

Plain TSV therefore requires a defined encoding.

The canonical text format should define a simple reversible escaping scheme.

Possible representation:

```text
\\    backslash
\t    tab
\n    newline
\xNN  arbitrary byte
```

Keeper should also support NUL-delimited machine output where appropriate:

```bash
keeper list --null
```

Human readability should not compromise correctness.

---

# Semantic Validation

Keeper should not implement decoders itself.

It orchestrates external validators.

Examples:

```text
JPEG       djpeg
RAW        LibRaw / dcraw_emu
PNG        pngcheck
video      ffmpeg
ZIP/7z     7z t
FLAC       flac -t
PDF        qpdf --check
SQLite     PRAGMA integrity_check
Git        git fsck
```

Validator rules should be configurable.

Possible configuration:

```toml
[validator.jpeg]
extensions = ["jpg", "jpeg"]
program = "djpeg"
args = ["{path}"]

[validator.video]
extensions = ["mp4", "mov", "mkv"]
program = "ffmpeg"
args = ["-v", "error", "-i", "{path}", "-f", "null", "-"]
```

Keeper must invoke programs directly, not through shell string interpolation.

This avoids quoting and injection problems.

---

# Content Integrity vs Semantic Integrity

These are separate dimensions.

## Content integrity

Question:

> Are these exactly the same bytes that were accepted before?

Mechanism:

```text
BLAKE3
```

## Semantic integrity

Question:

> Can the file still be parsed/decoded according to its format?

Mechanism:

```text
format-specific validator
```

A file may therefore be:

```text
HASH    FORMAT

PASS    PASS
PASS    FAIL
FAIL    PASS
FAIL    FAIL
```

Examples:

### PASS / FAIL

The file has not changed, but may have already been malformed when catalogued.

### FAIL / PASS

The file changed, but the new content is still structurally valid.

This could be a legitimate edit.

Keeper must not conflate these concepts.

---

# Catalog Consistency

Splitting the catalog into several text files creates database-like consistency concerns.

Keeper should explicitly enforce invariants.

## Invariant 1 — Path references

Every object referenced from `paths.tsv` must exist in `objects.tsv`.

```text
paths.object ∈ objects.hash
```

## Invariant 2 — Validation references

Every validation record must reference a known object.

```text
validation.object ∈ objects.hash
```

## Invariant 3 — Catalog generation

All authoritative files must belong to the same committed catalog generation.

## Invariant 4 — No implicit baseline changes

Observed filesystem changes must never mutate authoritative object mappings unless explicitly accepted.

---

# `MANIFEST`

The catalog should contain a small manifest defining the authoritative catalog generation.

Example concept:

```text
keeper-catalog-version	1
generation	42
created	2026-10-16T21:00:00Z

file	paths.tsv	b3:...
file	objects.tsv	b3:...
file	validation/raw.tsv	b3:...
file	validation/video.tsv	b3:...
```

Purpose:

- define the complete authoritative file set
- detect partial/corrupt catalog state
- identify format version
- make human inspection obvious
- allow Keeper to validate its own catalog before validating user data

A human should be able to open `MANIFEST` and immediately understand:

> These files constitute the complete catalog.

---

# Atomic Catalog Updates

Never modify authoritative tables piecemeal in place.

Instead:

```text
1. read existing catalog
2. scan / compute updates
3. construct complete new catalog generation
4. self-check it
5. fsync as needed
6. atomically replace old catalog with new generation
```

Conceptually:

```text
catalog/
.catalog.new/
```

then rename/swap on the same filesystem.

Result:

The authoritative catalog is always either:

```text
old valid generation
```

or:

```text
new valid generation
```

not a half-written mixture.

---

# Explicit Acceptance

Keeper needs a strong separation between:

```text
observe
```

and:

```text
accept
```

Potential commands:

```bash
keeper status
keeper verify
keeper accept path/to/file
keeper accept --all
```

or:

```bash
keeper update --accept-changes
```

Exact naming can be decided later.

Important semantic rule:

> No routine verification operation may silently rebaseline changed data.

---

# Operational Modes

Keeper should distinguish several cost levels.

## Status / incremental scan

Cheap.

Uses filesystem metadata to detect likely changes.

Conceptually:

```text
stat files
compare size/mtime/etc.
hash only new or changed candidates
```

Purpose:

```text
What appears to have changed since the catalog?
```

This is not a full corruption scrub.

---

## Verify

Check relevant files against known hashes.

Depending on CLI semantics, may still use cached/incremental knowledge.

Exact distinction between `status`, `verify`, and `scrub` should be made explicit during implementation.

---

## Scrub

Full physical integrity check.

Rule:

> Trust nothing. Read every byte.

For every expected object:

```text
read complete file
calculate BLAKE3
compare with catalog
record verification timestamp
```

This detects silent corruption.

---

## Deep scrub / semantic validation

Includes:

```text
full hash verification
+
format-specific validation
```

This is the expensive operation.

Possible command:

```bash
keeper scrub --deep
```

or separate:

```bash
keeper scrub
keeper validate
```

The latter may be clearer.

---

# Interruptibility and Resumability

Long-running operations must be safely interruptible.

This is a first-class feature.

Use cases:

- power failure
- reboot
- user wants to stop disk noise
- user wants to play a game
- laptop sleep
- validator crash
- temporary filesystem disappearance

Completed work should not be unnecessarily repeated.

---

# Per-object Verification State

The system should track timestamps such as:

```text
last_hash_verified_at
last_semantically_validated_at
```

Validation state belongs to the object hash.

Therefore rename does not invalidate validation.

Modification creates a new object with no validation history.

---

# Age-Based Work Selection

Keeper should support operations such as:

```bash
keeper scrub --older-than 30d
```

```bash
keeper validate --older-than 90d
```

```bash
keeper validate --never
```

```bash
keeper validate --failed
```

This allows rolling integrity maintenance.

---

# Budgeted Operations

Long-running work should support bounded execution.

Examples:

```bash
keeper scrub --budget 30m
keeper validate --budget 2h
keeper validate --max-files 5000
```

Behavior:

```text
perform useful work
checkpoint progress
stop cleanly at budget
```

This turns multi-hour maintenance into small sessions.

---

# Work Ordering

Default scheduling may prioritize:

```text
never checked
failed previously
oldest checked
everything else
```

Potential additional policies:

```text
--oldest-first
--largest-first
--type raw
--type video
```

Avoid premature complexity.

---

# Durable Progress

Do not rewrite large authoritative validation tables after every file.

Possible implementation:

```text
authoritative catalog
+
cache/work state
```

During a long validation session:

1. completed results accumulate in reconstructable working state
2. periodically create a new atomic catalog generation
3. checkpoint:
   - every N objects
   - every N seconds
   - on clean shutdown

Power loss may cause a small amount of repeated work, but must not corrupt authoritative state.

Avoid introducing complicated journaling unless later proven necessary.

---

# Performance Model

For modern systems, BLAKE3 computation is generally not the bottleneck.

Storage read speed usually dominates.

Approximate examples:

```text
external HDD      ~100–150 MB/s
SATA SSD          ~500 MB/s
NVMe              several GB/s
BLAKE3            several GB/s per modern CPU, higher with parallelism
```

Therefore:

> Optimize disk access patterns and number of passes before optimizing hash arithmetic.

---

# Full-Scrub Cost

A full scrub fundamentally requires reading all data.

Approximate physical minimum:

```text
1 TB at 120 MB/s   ≈ 2.3 hours
1 TB at 500 MB/s   ≈ 33 minutes
8 TB at 120 MB/s   ≈ 18.5 hours
```

Keeper cannot eliminate this cost.

Its job is to:

- avoid unnecessary extra reads
- resume interrupted work
- spread work over time
- make the cost visible

---

# Avoid Re-reading Data

Naive operation:

```text
read file to hash
read file again to validate
read file again during backup verification
```

can multiply physical I/O.

This matters far more than BLAKE CPU cost.

Design goal:

> Minimize full-data passes where practical.

However, external validators may need to open/read the file independently.

Keeper should not distort the architecture excessively to eliminate every reread.

---

# HDD vs SSD Concurrency

Storage parallelism and CPU parallelism are different.

## HDD

Multiple concurrent file reads can cause seek thrashing.

Default behavior should likely resemble:

```text
I/O readers: low, often 1
CPU hash workers: enough to keep up
validator concurrency: limited/configurable
```

## SSD/NVMe

Higher I/O concurrency may help, especially for many small files.

Potential controls:

```bash
--io-jobs N
--cpu-jobs N
--validator-jobs N
```

Possible convenience profiles later:

```bash
--io-profile hdd
--io-profile ssd
--io-profile nvme
```

Automatic detection is optional.

Explicit controls should always remain available.

---

# Many Small Files

Keeper must account for metadata and syscall overhead:

```text
open
stat
read
close
```

Millions of small files behave very differently from a few very large files.

Traversal implementation should therefore be profiled separately from BLAKE throughput.

---

# Performance Reporting

Long operations should make bottlenecks obvious.

Example:

```text
Scanned:          812.4 GiB
Read:             203.7 GiB
Hashed:           203.7 GiB
Validated:         81.2 GiB

Read rate:        118 MiB/s
Hash rate:        117 MiB/s
I/O wait:          high

Elapsed:          29m12s
Remaining:        ...
```

Keeper should help answer:

> Is Keeper slow, or is the storage slow?

---

# Incremental Operation

Normal operation should not rehash an unchanged multi-terabyte tree.

Keeper may use inexpensive metadata such as:

```text
path
size
mtime
```

to identify likely unchanged files.

Example:

```text
800 GB dataset
125,000 files

37 new
8 modified
4 removed
```

A normal status scan should inspect metadata and hash only relevant candidates.

A scrub explicitly ignores these shortcuts.

---

# Backup Use

Keeper does not perform backups itself initially.

Instead, it verifies them.

Example workflow:

```text
primary dataset
    |
    | rsync/restic/etc.
    v
backup disk
```

Then Keeper verifies that expected objects exist correctly.

Because content identity is path-independent, Keeper can recognize identical content despite renames.

Potential future commands:

```bash
keeper compare /mnt/data /mnt/backup
```

or:

```bash
keeper verify-backup ...
```

Do not initially reinvent `rsync`.

---

# Catalog Portability

Because authoritative metadata is plain text, a catalog can be:

```text
copied
rsynced
tarred
stored in Git
stored off-disk
opened in Vim
inspected years later
```

This is important because a catalog stored only beside the data is vulnerable to the same disk failure.

Users should be encouraged to copy/version `.keeper/catalog/` elsewhere.

---

# Catalog Versioning

Every catalog must declare a format version.

Example:

```text
keeper-catalog-version	1
```

Compatibility principle:

> Once Keeper writes a catalog version, future Keeper versions should remain able to read it or provide a lossless conversion path.

Where feasible, provide export into older/simple formats.

---

# Snapshots

Historical catalog snapshots are useful but not required for first implementation.

A snapshot records historical path → object mappings.

Example:

```text
snapshot A

foo.txt -> AAA
bar.jpg -> BBB
```

Later:

```text
snapshot B

foo.txt -> CCC
baz.jpg -> BBB
```

This naturally shows:

```text
foo.txt modified
bar.jpg renamed to baz.jpg
```

Snapshots should remain metadata-only.

Keeper does not store the historical file contents.

---

# Chunk Hashing — Future Feature

Whole-file BLAKE3 should be the initial model.

Later, optional chunk hashes could support:

- locating corruption within a large file
- identifying unaffected regions
- partial repair from another copy
- efficient verification of huge objects

Example:

```text
object   offset   length   chunk_hash
ABC      0        4MiB     ...
ABC      4MiB     4MiB     ...
```

This is explicitly deferred.

Do not complicate v1 around chunking.

---

# Filesystem Objects

Initial implementation should focus on regular files.

Later decisions are needed for:

- directories
- symlinks
- hard links
- ownership
- permissions
- xattrs
- sparse files
- alternate streams
- filesystem-specific metadata

Guiding principle:

> Keeper is primarily a content integrity system, not `tar`.

Only preserve filesystem metadata that serves Keeper's integrity goals.

---

# Hard Links

Identical hashes do not imply hard links.

Two independent files may have identical bytes.

If preserving hard-link topology becomes important, it requires path/filesystem metadata independent of content identity.

Defer unless there is a concrete need.

---

# CLI Direction

Tentative command vocabulary:

```bash
keeper init
keeper status
keeper scan
keeper verify
keeper scrub
keeper validate
keeper accept
keeper compare
keeper explain
keeper catalog check
```

Potential examples:

```bash
keeper init /mnt/data
```

```bash
keeper status
```

```bash
keeper scrub --older-than 30d --budget 1h
```

```bash
keeper validate --never
```

```bash
keeper validate --older-than 90d --budget 30m
```

```bash
keeper accept Photos/new.RAF
```

```bash
keeper explain Photos/foo.RAF
```

CLI semantics should be finalized before implementation.

Especially clarify:

```text
status
scan
verify
scrub
```

so that users understand exactly when bytes are physically reread.

---

# Suggested UX Model

A useful analogy is:

```text
Keeper catalog ≈ Git repository state
filesystem      ≈ working tree
```

Then:

```bash
keeper status
```

could report:

```text
M Photos/foo.RAF
    expected: b3:ABC...
    observed: b3:DEF...

R Photos/old.RAF -> Photos/new.RAF
    object: b3:GHI...

+ Documents/new.pdf

- Projects/old.zip
```

This should remain factual and conservative.

---

# Rust

Rust is the preferred implementation language.

Reasons:

- appropriate systems-level abstraction
- strong filesystem APIs
- excellent BLAKE3 library
- good control over concurrency
- low runtime overhead
- strong subprocess support
- suitable for large-data tools
- useful learning project
- single executable distribution

Avoid introducing async complexity unless benchmarks demonstrate a need.

A straightforward synchronous pipeline with controlled worker pools may be preferable.

---

# Implementation Stages

## Stage 0 — Format experiment

Before implementing Keeper broadly:

- define canonical path escaping
- define `paths.tsv`
- define `objects.tsv`
- define `MANIFEST`
- hand-create a tiny example catalog
- prove it is pleasant to inspect with:
  - Vim
  - grep
  - awk
  - sort
  - comm

Goal:

> Validate the human-facing data model before writing much code.

---

## Stage 1 — Genesis catalog

Implement:

```bash
keeper init
keeper status
```

Capabilities:

- walk regular files
- hash with BLAKE3
- create:
  - MANIFEST
  - paths.tsv
  - objects.tsv
- atomic catalog generation
- basic catalog self-check
- display summary

No validators yet.

---

## Stage 2 — Integrity verification

Implement:

```bash
keeper verify
keeper scrub
```

Capabilities:

- compare filesystem to catalog
- detect:
  - changed files
  - new files
  - missing files
  - exact-content renames
- distinguish expected vs observed hashes
- never auto-rebaseline
- full-byte scrub mode
- per-object verification timestamps
- resumable scrub

---

## Stage 3 — Explicit acceptance

Implement:

```bash
keeper accept ...
```

Capabilities:

- intentionally promote observed state into a new catalog generation
- preserve atomicity
- show exact changes before committing
- perhaps support:
  - individual paths
  - all changes

Potential safety model:

```text
status
review
accept
```

---

## Stage 4 — Semantic validation framework

Implement validator orchestration.

Capabilities:

- configured extension/type → validator mapping
- subprocess invocation without shell
- validation tables
- validator versions
- PASS / FAIL / ERROR
- object-based validation identity
- only new objects need initial validation

Initial validator set could focus on the recovery use case:

- JPEG
- RAF/RAW
- PNG
- ZIP/7z
- video
- PDF

---

## Stage 5 — Resumable maintenance

Implement:

```bash
--older-than
--never
--failed
--budget
--max-files
--oldest-first
```

Support both:

```text
scrub
validation
```

Checkpoint periodically using cache/work state + atomic catalog generations.

---

## Stage 6 — Backup comparison

Implement:

```bash
keeper compare
```

Possible capabilities:

- compare two Keeper catalogs
- identify exact matching objects
- identify missing backup objects
- tolerate path differences
- detect historical/renamed content
- produce machine-readable reports

Do not initially copy or repair files.

---

## Stage 7 — Performance tuning

Only after correctness.

Benchmark:

- HDD
- SATA SSD
- NVMe
- many small files
- large files
- parallel validators

Tune:

```text
I/O jobs
hash workers
validator jobs
filesystem traversal
checkpoint frequency
```

Do not optimize based on assumptions.

---

## Stage 8 — Optional future features

Possible later work:

- snapshots
- chunk hashes
- repair from known-good copies
- scheduled rolling scrub
- richer reports
- integration with backup tools
- catalog signing
- remote/offsite catalog verification
- filesystem-watch hints
- damaged-region localization

These are not required for Keeper v2's initial success.

---

# Critical Behavioral Rules

1. **Never silently accept changed content.**
2. **Never require opaque Keeper state to interpret the catalog.**
3. **Never make cache authoritative.**
4. **Never identify content by path alone.**
5. **Never identify paths using opaque artificial IDs when the path itself suffices.**
6. **Never use abbreviated hashes as canonical identity.**
7. **Never partially update authoritative catalog files in place.**
8. **Never assume semantic validity merely because the hash matches.**
9. **Never assume corruption merely because a hash changed.**
10. **Never hide what Keeper is doing when ordinary UNIX tools could reproduce the operation.**

---

# Design Heuristic

Use:

> Make everything as simple as possible, but no simpler.

Examples:

Good:

```text
separate path and object identity
```

because this removes real ambiguity.

Good:

```text
catalog/cache split
```

because it lowers cognitive load and protects portability.

Good:

```text
validation files by broad semantic domain
```

because they have distinct tooling and lifecycle.

Probably too complex initially:

```text
chunk-level trees
automatic repair
journaling engine
persistent daemon
```

---

# Mental Model

The full architecture can be summarized as:

```text
                 FILESYSTEM
                     |
                     | observe
                     v
               current bytes
                     |
                  BLAKE3
                     |
                     v
               observed object
                     |
              compare with
                     |
                     v
CATALOG:     path ----------> expected object
                                  |
                                  +--> semantic validation
                                  |
                                  +--> last scrub / validation
```

A path says:

> This location is expected to contain this object.

An object says:

> These exact bytes are the thing we know.

Validation says:

> These bytes were successfully interpreted by this validator at this time.

A scrub says:

> We physically reread those bytes and confirmed they still produce the expected object hash.

---

# Elevator Pitch

> **Keeper is a Git-like integrity catalog for ordinary filesystem data.**

It records which content object should exist at each path, identifies content using BLAKE3, delegates semantic integrity checking to format-specific tools, and keeps its authoritative state in simple human-readable text.

It does not store file contents and does not replace backup software.

Its purpose is to make answering:

> “Are my files still exactly what I think they are?”

easy, repeatable, inspectable, and independent of Keeper itself.

---

# First Real-World Use Case

The recovered DATA drive is an ideal initial test.

Proposed workflow:

```text
1. Restore recovered files onto new DATA storage.
2. Run semantic validation of important known formats.
3. Create Keeper Genesis catalog.
4. Scrub the restored corpus.
5. Copy files to rotating backup disks.
6. Create/compare Keeper catalogs on backups.
7. Periodically scrub backup objects based on age.
```

This provides both:

- immediate confidence in the recovered dataset
- a realistic multi-hundred-GB / ~1-TB corpus for profiling Keeper v2

The project should therefore be developed against real recovery data rather than synthetic benchmarks alone.