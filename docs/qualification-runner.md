# Repeatable sample qualification

The maintained runner is [`tools/qualify.py`](../tools/qualify.py), using Python 3
and the normal Rust toolchain. It operates on **one source filesystem at a time**.
See [the qualification procedure](qualification.md) for the larger readiness gate.

```sh
# Both destinations must NOT exist; their parent directories must already exist.
# Use a NEW name for every run, and stop source writers while copying.
python -B tools/qualify.py prepare \
  --source /mnt/example \
  --workspace /mnt/example/keeper-qualification-YYYYMMDD \
  --report "$HOME/keeper-qualification-results/example-YYYYMMDD" \
  --subtree photos --subtree documents --subtree videos

python -B tools/qualify.py run \
  --workspace /mnt/example/keeper-qualification-YYYYMMDD \
  --evict-sample-cache

python -B tools/qualify.py help prepare
python -B tools/qualify.py help run
```

## Preparation and safety boundaries

Preparation selects a deterministic bounded mix (default up to 1 GiB and 1,200
files per sample, maximum individual file 512 MiB), with a metadata scan limit per
explicit subtree. Increase `--budget-mib`, `--max-files`, `--max-file-mib` and
`--scan-limit` explicitly for larger trials. `--selection balanced` (the default)
starts with a small/medium/large-file mix, then fills remaining capacity without
hidden per-size-class ceilings. `--selection large-byte` prefers largest files;
`--selection many-file` prefers smallest files to stress file-count overhead.
Size ties use deterministic path hashes. All modes obey the byte/file limits and
only select from the examined candidates; none is a uniform percentage sample.
Many-file results can overrepresent empty/tiny files: inspect the manifest before
interpreting throughput. Raise `run --timeout` (default 600 seconds per command)
for larger trials. Space and read costs include two copies and multiple full passes. It copies source bytes once into two independent physical files;
no reflinks or hard links to originals. The workspace must be on the source
filesystem. Reports must be outside the source/workspace; choose another device.

Source `.keeper` metadata and root `.keeperignore` are not copied as sample data.
The original exclusion policy is preserved, not implicitly applied as a sampling
rule: selected regular files and bounded scan coverage are listed in the report.
The tool does not qualify omitted original paths or silently follow symlinks.
New qualification directories appear as additions in whole-drive checks; the tool
does not edit the original ignore policy to hide them.

`run` verifies prepared copies, builds release, runs the complete Rust suite with
`TMPDIR` inside the workspace, adds explicit filename/hard-link probes, and exercises
independent init, catalog checks, recorded/live comparison, fresh/checkpointed status,
deliberate drift, baseline immutability and off-device exports. A separate sparse
2 GiB file makes SIGTERM/SIGKILL interruption tests independent of sample size.
Allow enough space if the filesystem allocates sparse files eagerly. Deliberate
sample edits are restored; samples, sparse files and logs are retained for inspection.
Never use originals as the workspace. Marker/identity checks protect against
accidental target confusion, not a malicious process concurrently replacing paths.

## Reports and repetition

Reports contain command lines, exact exit codes, timings, sampled `/proc` VmHWM,
conservative `wait4` RSS upper bounds, environment, source/binary fingerprints
(including uncommitted files), selection manifest, capability failures,
metadata-timing probes and catalog exports. Unsupported filename probes
and suite failures remain visible and prevent a full pass. `--skip-suite` explicitly
produces an **incomplete** smoke result. Exit codes: 0 completed passing sample
checks (or explicitly incomplete smoke), 1 failed checks/capabilities, 2 runner error.
An interrupted/errored run retains partial logs and must not be treated as complete.
User interruption exits 130. Build failures stop the run rather than testing a stale
binary.

Optional cache eviction is advisory `POSIX_FADV_DONTNEED` on disposable files only,
never a global cache drop or a guarantee of cold controller/device caches.
`probe --workspace ...` reruns timestamp diagnostics independently; add
`--control NEW_DIRECTORY` for a comparison on another filesystem. Use a new
workspace/report for another full run; existing catalogs are never quietly reused
as a new initialization experiment. No automatic cleanup, mount changes, installation,
power-cut tests or changes to originals are performed.

Test the tooling itself with:

```sh
python -B -m unittest discover -s tools -p 'test_*.py'
```

Repeat qualification after relevant Keeper, kernel/driver, mount-option or storage
changes, and after fixes to the safety policy. Preserve exact environment and dirty
working-tree state with each run. Previous sample successes are not certification
of a new configuration. Reports may contain private filenames; do not commit them
or publish them without review.

## NTFS findings and current policy

Initial target samples on ext4/SATA SSD passed 98 tests and ordinary workflows.
The same-sized ntfs3/USB HDD sample passed ordinary workflows but failed namespace
and intermittent content-mutation race checks. Independent probes reproduced rapid
same-size edits with identical device/inode/size/mtime/ctime, including edits without
restoring mtime. Fingerprints stayed identical after fsync and elapsed clock ticks.
A local Btrfs control did not reproduce those collisions.

The matching Linux 7.2.11 reference uses coarse automatic timestamps for ntfs3
(no `FS_MGTIME`), while ext4/Btrfs opt into multigrain timestamps. The tested kernel
uses HZ=1000; NTFS's 100 ns storage precision is a separate property. HDD/USB latency
can affect timing and throughput but does not explain this metadata limitation.
Do not make tests pass by sleeping or claim a second hash guarantees a snapshot.

Current mitigations re-enumerate selected namespace membership independently of
timestamps and disable checkpoint reuse per file on NTFS (both kernel driver magic
values), FAT, exFAT and FUSE. FUSE is conservative because it does not identify its
backing implementation (including ntfs-3g). Filesystem inspection errors fail closed.
Files on nested mounts receive the same restriction, even under an unrestricted root.
This known-risk restriction is not certification of every other filesystem.

This does **not** fix all concurrent-content races: stop writers throughout every
operation and resumed session, or use a genuinely read-only snapshot. Catalog and
checkpoint encodings are unchanged. No reformatting or loss of existing hashes is
required. A post-mitigation DATA2 rerun passed all 47 unit tests and all supported-name
workflow checks, including zero checkpoint reuse after interruption. The complete
103-test suite had 100 passes and three unsupported-byte-filename creation failures.
Thus the strict arbitrary-byte filename suite still cannot fully pass on the UTF-8
ntfs3 mount: unsupported names fail visibly rather than being substituted. This is
not a claim that concurrent content mutation has become safe.
