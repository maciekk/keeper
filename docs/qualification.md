# Filesystem qualification and resource measurements

Resume is implemented; target ext4 and NTFS samples have exposed important limits,
not established blanket whole-drive readiness. No sample establishes power-loss
durability or terabyte-scale readiness. Keep originals and independent backups.
Use only disposable samples here.

For repeatable preparation, tests, metadata probes, reports and off-device exports,
use the maintained [qualification runner](qualification-runner.md). It also records
the NTFS findings and conservative checkpoint policy.

## Qualify each intended mount

1. Choose a dedicated disposable directory on the intended filesystem. Record
   `findmnt -T /chosen/directory`, kernel, device type, mount options, free space,
   Keeper revision and `rustc --version`. Choose explicit corpus roots: unsupported
   system objects are errors, not implicit exclusions. Choose any explicit
   [`.keeperignore`](ignore.md) policy before starting scans; qualifying selected
   data does not verify excluded subtrees.
2. Run the tests with temporary files **on that mount**:

   ```sh
   # This directory must already exist, be writable and contain no valuable data.
   TMPDIR=/chosen/disposable/directory cargo test --locked
   ```

   Tests cover encoding, reads, locking, sync/publication, no-replace behavior,
   drift, checkpoint corruption, cancellation and process death. Resume interruption
   tests create 2 GiB sparse files; ensure the target supports them and has enough
   capacity if it allocates eagerly. Unsupported capabilities must fail, never
   justify weakening the publication protocol. Test on copies, not original data.
3. Use a representative disposable corpus with large files, many small files,
   duplicates, hard links where supported, and unusual filenames. Run preflight,
   `init --resume`, `catalog check`, recorded comparison against an independently
   initialized copy, plain status, and `status --resume`. Cancel in a large file,
   repeat the same resumable command, and inspect the reused/new-read counts.
4. Make deliberate content changes (including same-size/restored-mtime changes),
   additions and deletions. Confirm drift and an unchanged catalog. Copy catalog
   metadata off-device; inspect with the format-1 reader/manual checks.
5. Record results per filesystem. Process death tests are **not** power-cut tests.
   Genuine crash/power-loss experiments require a separate disposable environment,
   recovery checks and explicit hardware/filesystem assumptions.

Do not call a mount qualified based on preflight alone. Preflight reads metadata,
not file contents, and does not test publication or durability support.

## DATA SSD scale samples (2026-11-09)

Two independent disposable-sample trials on DATA (ext4, `rw,relatime`, WD Blue
SA510 SATA SSD), Linux 7.2.11-arch1-1, rustc 1.99.1, release build at `2aedc00`
plus the sampling-tool changes recorded in each report's provenance. Both trials
passed all 32 recorded command checks, including the complete Rust test suite on
the mount, independent catalogs, physical/recorded comparisons, deliberate drift,
immutable baselines, off-device exports, and SIGTERM/SIGKILL resume checks. No
filename/hard-link capability limitations were reported.

| Measurement | Large-byte sample | Many-file sample |
| --- | ---: | ---: |
| Files per independent copy | 23 | 100,000 |
| Data per copy | 32 GiB | 70.06 MiB |
| Preparation (both copies) | 243.90 s | 771.98 s |
| Preflight A | 0.002 s | 0.787 s |
| Init A, checkpointed | 65.454 s | 3.701 s |
| Fresh status A | 64.607 s | 3.356 s |
| Checkpointed status A (new session) | 65.126 s | 3.667 s |
| Live comparison (both copies) | 129.101 s | 6.285 s |
| Advisory-evicted status A | 65.658 s | 13.954 s |
| Rewarmed status A | 64.645 s | 3.384 s |
| Sampled peak RSS, fresh status A | 3.59 MiB | 129.59 MiB |
| Sampled peak RSS, live comparison | 3.61 MiB | 160.54 MiB |

Large-byte selection used up to 8 GiB individual files from three explicitly
selected subtrees. Many-file selection preferred the smallest eligible files from
three subtrees: this is a metadata/file-count workload, not a representative
average file-size distribution. Preparation syncs both copies per file; it is not
Keeper's scan time. Measurements are single runs. Cache eviction used advisory
per-file `POSIX_FADV_DONTNEED`, not global cache drops or guaranteed cold storage.
Sampled `/proc` peaks can miss short-lived allocations (especially preflight);
see the exact per-command RSS measurements and conservative wait4 bounds in the
reports rather than extrapolating a whole-drive memory bound.

Private reports are retained off-device under
`~/keeper-qualification-results/DATA-scale-20261109-174056/{large-byte,many-file}/`
with manifests, source/binary fingerprints, environment, command logs, and exports.
These successful samples support a monitored whole-drive verification attempt on
this configuration; they do not establish whole-drive integrity, power-loss
durability, or qualification of DATA2/other devices. Preserve independent backups.

## Local smoke measurements

Release build, local `/home` Btrfs (`ssd,compress=zstd:3`), warm/newly written data:

| Sample | Command | Wall seconds | Peak RSS upper bound (KiB) |
| --- | --- | ---: | ---: |
| 1 × 512 MiB file | preflight | 0.001 | 17,984 |
| same | init --resume | 0.111 | 17,984 |
| same | status | 0.089 | 17,984 |
| same | status --resume (new session) | 0.092 | 17,984 |
| 10,000 × 4 KiB files | preflight | 0.019 | 18,368 |
| same | init --resume | 10.468 | 18,368 |
| same | status | 0.115 | 18,368 |
| same | status --resume (new session) | 10.975 | 18,368 |

Measured via Python `time.monotonic()` and `os.wait4(...).ru_maxrss`; RSS includes
launcher/pre-exec high-water overhead, so these are upper bounds, not an accurate
per-structure memory profile. Single runs, not statistical benchmarks. Large-file
bytes repeated `bytes(range(256))`, small files repeated `x`; both are highly
compressible, cache-friendly and **not representative of photo/video drives**.
No caches were dropped, no original data was read, and no target backup devices
were tested. Both status modes started new sessions (no reused file observations).

The useful finding is checkpoint cost: about 1 ms per small file for per-file
fsync on this setup, dramatically more than plain status. Do not extrapolate the
large-file rate to disks or tune reader concurrency from these numbers.

For representative runs use release builds and `/usr/bin/time -v` (if installed)
or a comparable per-process monitor. Record traversal time, hashing time, peak RSS,
file counts/bytes, checkpoint cost and resumed/repeated work separately. Measure
cold and warm cache conditions explicitly, on each relevant HDD/SSD and realistic
corpus sizes. Include a metadata-heavy corpus large enough to expose inventory
memory growth. Current RAM use grows with corpus/journal size; these tiny local
samples do not bound whole-drive memory use. Keep concurrency changes deferred
until measurements show the bottleneck.
