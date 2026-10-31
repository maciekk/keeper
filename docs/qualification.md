# Filesystem qualification and resource measurements

Resume is implemented, but **actual target drives have not been qualified**.
No local smoke test establishes power-loss durability or terabyte-scale readiness.
Keep originals and independent backups. Use only disposable samples here.

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
