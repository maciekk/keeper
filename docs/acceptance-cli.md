# Everyday acceptance and snapshots

Implemented: short acceptance commands, one confirmation, automatic metadata-only
migration, immutable history, named snapshots and read-only previews. Advanced
saved plans remain available; see [intent formats and recovery](intent-v1.md).

## Principle

Users authorize a kind of expected change, not manage a storage protocol. Borrow
Git's short vocabulary without introducing a staging/commit workflow. One command
discovers selected changes, summarizes them, asks for confirmation, revalidates,
and saves history. No required plan files or routine migration commands.

Keeper records expectations, not file contents. It is not version control, a
synchronizer, or a restore tool. No command below modifies dataset files.

## Command set

Run anywhere inside a dataset; short commands discover its nearest enclosing
`.keeper`. Use `--root ROOT` mainly for scripts or operating from elsewhere.

| Command | Meaning |
| --- | --- |
| `keeper status` | Inspect differences; never accept them. A full-byte check by default. |
| `keeper add [PATH...]` | Accept new files only. No paths: additions in the current subtree. Directories: recursive additions. |
| `keeper rm [PATH...]` | Accept already-deleted files only. No paths: observed deletions in the current subtree. Directories/prefixes: missing tracked descendants. |
| `keeper update PATH...` | Accept deliberate edits to named tracked files. Requires files; no directory or implicit bulk update. |
| `keeper mv OLD NEW` | Accept one already-performed file move, with unchanged contents. |
| `keeper snap -m MESSAGE [PATH...]` | Review a mixed batch of additions, modifications and deletions; record it as one named snapshot. No paths: the current subtree. |
| `keeper snap --mark -m MESSAGE` | Name the current baseline without reading or accepting live-file changes. |
| `keeper log` | List acceptance history and snapshot messages, newest first; no dataset bytes read. |

```sh
cd /photos
keeper snap --mark -m "Before today's session"
# Import from camera, edit older photos, delete blurry ones with normal tools.
keeper snap -m "Imports, edits and cleanup finished"
keeper log

# Or accept just one class of change:
keeper add
keeper add 2026/trip
keeper rm
keeper rm 2026/blurry-shoot
keeper update edited.jpg
keeper mv old.jpg archive/old.jpg

# Same operation without changing the shell's current directory:
keeper --root /photos add
```

### What a snapshot means

A snapshot is a named historical stopping point: a complete immutable map of
expected paths, hashes and sizes, with your message and a wall-clock record time.
Previous expectations remain available. A mixed `snap` may also accept selected
changes; a marker records only existing expectations. Repeating a snapshot with
no changes still records a distinct stopping point.

**Snapshots do not store file contents.** Old expectations can identify which bytes
are needed, but restoring deleted files or undoing edits requires independent
versioned backups. No checkout/restore, history comparison command, or permanent
"verified" status is implied. `log` lists history; richer history queries remain
future work.

Detecting modifications reliably requires reading bytes. A regular `snap` reads
selected current files (the current subtree by default) to find edits. Changed/new
selected bytes are reread after authorization. Even a no-change regular `snap`
pays the initial read cost; it cannot safely infer no edits from timestamps.
Use **`snap --mark`** for a quick before-session marker with no dataset-content
reads. A marker does not certify the tree's current health or accept pending edits.
Neither operation creates durable per-file verification history; resume state
remains disposable. Stop writers throughout discovery, confirmation and publication.

### Deliberate differences from Git

- `add` never updates tracked files' expectations. Directory/bulk selection leaves
  them alone; explicitly naming a tracked file is an error directing you to `update`.
- `rm` never deletes physical files. A tracked path must already be absent. A
  still-present directory, symlink (even dangling), unreadability or I/O failure is
  not an acceptable deletion. Bulk selection leaves present tracked paths alone.
- `mv` never moves physical files. OLD must be tracked and absent; NEW must be an
  untracked regular file whose freshly read hash and size equal OLD's expectation.
  Remove/add publish atomically. A simultaneous edit is rejected, not silently
  authorized. Directory moves and inferred renames are not supported.
- `update` accepts modifications only, not additions or removals. An unchanged
  named file is a no-op. `snap` explicitly authorizes a reviewed mixed batch.
- No staging area or separate commit is required. Narrow acceptance commands
  immediately create history; `snap` adds named session boundaries to that history.

## Roots, selectors, and scope

Without `--root`, `add`, `rm`, `update`, `mv`, `snap` and `log` search upward from
the current directory for the nearest `.keeper`. A nearest marker is a hard boundary:
corrupt, incomplete, unreadable, symlink or non-directory metadata causes an error,
never fallback to an ancestor dataset. Nested datasets therefore use their own
baseline. If none exists, fail with guidance to initialize or specify `--root`.

Implicit acceptance selectors are **current-directory-relative**, not relative to
the discovered root. Bare `add`, `rm`, or regular `snap` selects the current subtree,
never silently the whole enclosing dataset. For example, in `/photos/2026`,
`keeper add trip` accepts additions under `/photos/2026/trip` using `/photos`'s
baseline; bare `keeper add` considers only `/photos/2026`. Inherited expectations
outside that selection remain unchanged in the complete new revision. `log` lists
the discovered dataset's history; `snap --mark` names its current expectations
without checking any live files.

`--root ROOT` explicitly selects the dataset and bypasses discovery. Selectors are
then **dataset-relative**, and a bare command selects that entire dataset. Relative
ROOT values resolve from the shell's directory. Neither mode changes the working
directory; external paths (plans, exports) retain their ordinary shell-relative
meaning. `--root` is useful for automation or operating from outside the dataset.

`--root` also works with `init`, `status`, `preflight`, `catalog check`, and
`catalog migrate`. These legacy explicit-root commands keep their existing default
of the current directory; discovery/subtree rebasing applies to the short commands
above, not to `init` or full-tree checks. Existing forms such as `keeper status /photos` remain supported;
do not supply both `--root` and a positional ROOT. `compare`, `catalog export`, and
advanced `accept` commands retain their explicit root/path arguments and reject
`--root` rather than silently ignoring it. There is currently no `-C` alias.

For new acceptance commands, positional paths are selectors within the root, not
another ROOT argument. Paths are literal: do not pass `/photos` as an `add`
selector. `add .` and `rm .` select the current subtree (or explicit root).
`./album/` and parent-relative paths within the dataset are supported. Escaping
the dataset, crossing symlinks, and absolute selectors are rejected. Directory matching
respects component boundaries (`trip` never includes `trip-old`). Overlapping
selectors are deduplicated. Use `--` for names beginning with `-`:

```sh
keeper --root /photos add -- -unusual-name.jpg
```

No internal glob expansion, symlink following, scope changes, or automatic ignore
rules. Reports escape arbitrary filename bytes. Explicit ignored/configuration
selectors and unknown selectors fail clearly. An existing directory with no
applicable changes, or an empty bulk selection, is a successful no-op. A missing
`rm` directory selector is resolved against tracked descendants.

## Confirmation and output

Reports show the dataset, action counts, sizes and directory summaries. `mv`
shows both paths. Other expectations remain untouched. One prompt defaults to no:

```text
Dataset: /photos
  2026/trip: 100 added, 0 updated, 0 removed
100 added, 0 updated, 0 removed; 2.4 GiB of selected new bytes.
Other expectations remain unchanged. Dataset files are never moved, deleted or copied.
Accept these changes? [y/N]
```

- `--long`: list individual selected paths and exact old/new hashes and sizes.
- `--dry-run`: discover/report without prompting, migrating or writing metadata.
  It may hash selected files; it is not necessarily a cheap metadata-only check.
- `-y`, `--yes`: authorize without prompting for scripts. Never bypass validation.

Noninteractive acceptance requires `--yes` or `--dry-run`; piped input/EOF is not
consent. Preview, declined confirmation and ordinary no-op never migrate or create
metadata, including `LOCK`. A declined prompt or no-op exits 0. Operational errors,
stale batches and signal cancellation exit 2; `status` retains exit 1 for differences.
A named snapshot is an intentional history entry even with zero file changes,
not an ordinary no-op. Messages must be nonempty. `--mark` accepts no path selectors.

No preliminary `status` is required. A previous report is not authority to approve
subsequently changed bytes. A normal `snap` can be expensive; use its scope selectors
or `--mark` deliberately, not as silent substitutes for physical verification.

## Safety and recovery

Discovery pins the exact parent and selected actions without writing metadata.
After confirmation, acquire the dataset writer lock, reject a changed parent,
and freshly revalidate approved hashes and absences before publishing. Do not add
files that appeared while the prompt was open to the approved batch. Failed/stale
actions reject the whole batch; there is no automatic rebase or partial acceptance.
Concurrent readers keep their pinned immutable expectations.

Bulk addition/snapshot discovery traverses only selected directories; incomplete
inventories, unsupported selected objects, permission errors and I/O failures fail
closed. Removal checks tracked paths directly, never interpreting traversal failure
as absence. Unselected tracked files are not hashed for add/rm; namespace consistency
and exact exclusion policy remain required.

Absence alone cannot prove an intentional deletion: an unmounted subtree can look
like a removed directory. Check the summary and intended mounts before authorizing
removals. No claim of detecting all missing mounts/concurrent edits is made.

A confirmed nonempty acceptance or named snapshot on a valid legacy catalog
migrates metadata automatically, retaining the original unchanged. The normal
confirmation/report warns that older Keeper builds must not be used afterward;
there is no second routine migration command/prompt. Missing baselines still need
`init`. Incomplete/corrupt intent stores fail closed with recovery guidance.

Migration and acceptance are distinct durable steps: a failure can leave a
successfully migrated but logically unchanged baseline. Before the acceptance
reference switch, failures preserve old expectations; a final directory-sync failure
after switching is uncertain publication, not promised rollback. Inspect
`catalog check` before retrying; see [publication/recovery details](intent-v1.md).
No automatic cleanup or history deletion is performed.

`accept plan`, `accept show`, `accept apply` and explicit `catalog migrate` remain
advanced workflows for separate review, audit, mixed actions and migration recovery.
Users of ordinary commands need not manage plan filenames, revision IDs or checksums.
The existing resume checkpoint files are **resume state**, not named snapshots or
durable historical verification evidence.
