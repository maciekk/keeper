# Checkpointed operations (work formats 1 and 2)

```sh
keeper init --resume /path/to/dataset
# Ctrl-C, then repeat exactly the same command:
keeper init --resume /path/to/dataset

keeper status --resume /path/to/dataset
# Ctrl-C, then repeat to continue this verification session.

# Less frequent durability flushes for a slow disk (positive whole seconds):
keeper init --resume --checkpoint-interval 10 /path/to/dataset

# A NEW physical check, ignoring all work state:
keeper status /path/to/dataset
```

`--resume` starts a session when none exists and continues it otherwise. It is
opt-in on both commands; `compare --live` does not yet support checkpoints. Plain
`init` and `status` still read every file. Plain status and all comparisons remain
read-only; **`status --resume` writes disposable work state**, not the baseline.
A completed status with differences still exits 1; operational errors and clean
cancellation exit 2. SIGINT and SIGTERM request cooperative cancellation between
reads/traversal steps. Blocked kernel I/O may delay handling. SIGKILL cannot be
handled, but already-synchronized records survive normal process death.

## Meaning of a resumed pass

A session consists of per-path physical observations collected over an interval,
possibly across process invocations. Output always identifies the session and says
**NOT a fresh full scrub**, with counts of reused versus newly read paths. Keep
writers stopped for the *entire session*, including pauses, or prefer a read-only
snapshot. No session is an instantaneous snapshot.

Every invocation repeats metadata preflight. Reuse requires the same canonical
root path, root device/inode, operation, work-format version, and (for status) exact
validated baseline contents. It also requires the exact `.keeperignore` policy,
including comments, to remain unchanged. Adding or removing the file changes scope;
see [ignore-policy safety and recovery](ignore.md#baselines-and-resumable-sessions).
Renaming/moving the root, copying work to another
root, a changed baseline, or an unknown version causes refusal. Root device/inode
are local identity checks, not a persistent filesystem UUID or hostile-clone defense;
remount/device-number changes can require restarting.

Each completed **path**, not each content hash, has its own observation. A matching
device, inode, size, nanosecond mtime and ctime can permit reuse, subject to the
filesystem restrictions below. Changed fingerprints are reread, newly discovered
paths are read, and removed paths are not resurrected from work records. Selected
namespace membership is re-enumerated before/after hashing, independently of
directory timestamps; all inventoried file identities are also checked again.
These streaming rechecks add traversal/metadata cost, not another full inventory.

**NTFS (both kernel drivers), FAT, exFAT and FUSE do not reuse checkpointed file
reads.** Every invocation rehashes their files, including files on nested mounts
inside an otherwise unrestricted root. Work/session identity and corruption checks
still apply; cancellation/recovery remains available, but saved reads do not reduce
the next pass's byte-reading cost. FUSE is conservative because its filesystem
identity does not identify the backing implementation (including ntfs-3g).
Filesystem inspection errors fail closed. This known-risk restriction is not a
certification of every other filesystem. Catalog/work formats are unchanged.

Equal metadata is not proof of unchanged contents. On the tested `ntfs3` mount,
rapid same-size ordinary edits, even without restoring mtime, retained the entire
fingerprint. Fsync and waiting afterward did not reveal the change. NTFS's 100 ns
stored timestamp precision does not imply distinct automatic timestamps for each
write. See [qualification findings](qualification-runner.md#ntfs-findings-and-current-policy).
No mode detects **silent corruption without reading the affected bytes**, malicious
timestamp manipulation, or every concurrent mutation. A reused record means “bytes read earlier in this session,”
never “bytes physically read now.” Run plain `status` for new full-byte evidence.
Neither mode accepts changes or overwrites an existing baseline. Resumable init
explicitly assembles its genesis baseline from these session observations; use
plain init instead if fresh uninterrupted reads are required.

## Work state and recovery

Work files are `.keeper/init.work` and `.keeper/status.work`, under the same
exclusive `.keeper/LOCK` used for initialization. A work file contains:

- A checksummed header: `keeper-checkpoint`, version `1` for unscoped sessions or
  `2` when `.keeperignore` exists, operation, escaped canonical root, device, inode,
  baseline fingerprint (or `genesis`), then (for version 2) the policy's full BLAKE3
  ID, and session ID. Version 1 retains its original encoding unchanged.
- Append-only checksummed rows: escaped relative path, physical fingerprint
  (`dev:ino:size:mtime:mtime_nsec:ctime:ctime_nsec`), full BLAKE3 ID, byte size,
  observation completion time in Unix nanoseconds.
- Each checksum is lowercase BLAKE3 hex over the preceding tab-separated payload.
  A row ends with a tab, its checksum, and newline. The session ID starts with its
  creation time in Unix nanoseconds and includes a random temporary-file suffix.
  Times are wall-clock observations, not a trusted monotonic history.

Header creation is synced and published without replacement. Completed-file rows
are appended immediately; durability flushes are batched after **five seconds since
the previous flush** by default, checked at file completion. Use
`--checkpoint-interval SECONDS` with `init --resume` or `status --resume` to change
this to a positive whole number of seconds (for example, 5 or 10). There is no
file-count trigger. The interval is a runtime setting, not part of the checkpoint
identity; it can be changed when resuming an existing session without discarding
work. Longer intervals reduce flush overhead but increase potential rereads after
power loss. There is no background timer: hashing a large file or waiting in I/O
can delay the next flush. Normal process cancellation or death leaves complete
appends in the kernel cache; a system crash or power loss can lose the unsynced
batch, causing those files to be reread. The current file is always reread after
interruption: there is no partial-file BLAKE3-state serialization. Preflight and final metadata
rechecks also repeat; resume avoids completed content reads, not traversal costs.
A partial trailing row is discarded and the truncation synced on reopen. Complete
bad checksums, invalid fields, unsafe paths, and wrong identities fail closed.
Checksums detect accidental damage, not coordinated forgery. Removing entire rows
merely causes rereads. Latest complete observations for a path take precedence.

Success removes/syncs the work file after publication (init) or reporting (status).
A crash after catalog publication but before work cleanup leaves a harmless stale
`init.work`; init still refuses to replace that catalog. Use `catalog check` and
plain `status` to inspect it. Do not remove a catalog to retry initialization.

To abandon/restart a session, **stop all Keeper processes first**, remove only its
`.keeper/init.work` or `.keeper/status.work`, and rerun. Never remove `.keeper/catalog`
or replace/unlink `LOCK` to clear work. Failed initial header publication may leave
`.keeper/.work-*` temporary files after a hard kill; these are disposable too.
Baseline copies remain independent of this cache: work records are not durable
verification history and should not be used as a long-term “last scrub” database.

## Costs and limits

Hashing is synchronous with a 128 KiB buffer. Catalogs, inventories, and parsed
work records are in RAM; loading work also temporarily holds its text. Memory is
O(paths + directories + journal size), not bounded independently of corpus size.
Repeated interrupted sessions with changed files grow the append-only journal;
remove disposable work and restart if this becomes excessive. There is no journal
compaction yet. Final file-stat rechecks add an O(paths) metadata pass.

Batched fsync avoids a durability flush per small file, but still writes work to
the dataset drive. After power loss, lost content work can include the unsynced
batch and one in-flight file, not a fixed byte/time budget: one very large file
may still require hours to reread. Actual power-loss behavior depends on
filesystem/hardware, not just process-kill tests.

See [qualification and measurements](qualification.md). Target-drive qualification
and representative measurements remain required before a definitive whole-drive
baseline; implementation of resume alone does not satisfy that readiness gate.
