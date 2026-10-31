# Explicit scan scope with `.keeperignore`

A dataset root may contain one `.keeperignore` file. Its exclusions apply to
preflight, initialization, status, and live/metadata-only comparison. Catalog-only
comparison uses each catalog's saved policy, not current configuration files.
There are no automatic housekeeping exclusions.

For example, create `/mnt/data/.keeperignore` with:

```gitignore
# Filesystem recovery directory, deliberately outside this catalog.
/lost+found/
```

Then check the selected scope before indexing:

```sh
keeper preflight /mnt/data
keeper init --resume /mnt/data
```

Keeper skips the matching directory before listing its contents. This avoids the
usual `lost+found` permission error without changing its permissions or treating
an unreadable directory as empty. Excluded entries and subtree roots are counted,
but files and bytes inside them are not inspected or estimated. Reports disclose
the patterns and exclusions; they are not “junk” classifications.

Do not overwrite an existing ignore file blindly. For comparable drive baselines,
choose the same policy on both roots **before** initialization or a resumable
session. Exact source bytes—including comments—identify the policy, so copying
the file between roots is preferable to independently writing equivalent rules.
Matching selected data never certifies excluded data or whole-drive equality.

## Supported syntax (keeperignore-1)

This is a deliberately limited Git-style syntax, not a full Git ignore engine:

| Pattern | Meaning |
| --- | --- |
| `# comment` or an empty line | No rule |
| `/lost+found/` | Only this directory at the dataset root, and its descendants |
| `cache/` | Directories named `cache` at any depth |
| `*.tmp` | Matching names at any depth, including matching directory subtrees |
| `photos/*.tmp` | Matching immediate children of the root's `photos` directory |
| `photos/**/scratch?.jpg` | Matches through zero or more directory levels |
| `photos/**` | All descendants of the root's `photos` directory |

- A leading `/` anchors a pattern to the dataset root. A pattern containing an
  internal `/` is also root-relative. Otherwise it matches a basename at any depth.
- A trailing `/` requires a real directory, not a symlink. Excluded directories
  prune their whole subtree; directory metadata is not otherwise cataloged.
- `*` matches zero or more bytes within one path component; `?` matches one byte.
  `**` must be an entire component. In the middle it matches zero or more levels;
  a final `/**` matches descendants, not the parent itself.
- Trailing spaces/tabs and CRLF line endings are tolerated. Leading spaces are
  literal. Comments start with `#` in column one.
- **Negation (`!`), backslash escapes, and character classes (`[...]`) are not
  supported.** Invalid/unsupported patterns are errors, not silently weakened rules.
  Names containing these syntax characters cannot be expressed literally yet.
- The file must be regular UTF-8 text, at most 1 MiB, not a symlink. Patterns match
  Unix path bytes, including arbitrary non-UTF-8 filenames through wildcards.
- Only the root file is configuration. Nested `.keeperignore` files are ordinary
  data unless explicitly excluded. Root `.keeperignore` is configuration and is
  not cataloged as a dataset file. Root `.keeper/` remains reserved regardless of
  rules; rules cannot hide an invalid `.keeper` object.

## Baselines and resumable sessions

When `.keeperignore` exists—even if empty—new catalogs use
[format 2](catalog-v2.md), preserving its exact bytes as checksummed `scope.ignore`.
The rule-language identifier is stored too. No ignore file means the unchanged
format-1 layout and no explicit exclusion policy. Both versions remain readable;
existing unscoped baselines need no rehashing or conversion.

`status` requires the current ignore file to match the recorded policy exactly.
Adding, removing, or editing it is an operational error before hashing; it cannot
silently turn a tracked file into an ignored one. Restore the recorded policy to
continue checking. There is no command to accept a new scope on an existing
baseline yet. **Never delete a catalog to silence scope or content differences.**

Resumable sessions pin the same policy. Editing it invalidates reuse even if no
included file changed. Keeper also rechecks policy bytes across traversal and
hashing. These checks detect common changes, not an adversary or a filesystem
snapshot; keep writers stopped throughout pauses as well.

If adding the ignore file after a failed preflight has left `init.work`:

1. Stop all Keeper processes using this root.
2. Decide and save the policy.
3. To abandon the old session, remove **only** `.keeper/init.work`.
4. Repeat preflight and `init --resume`.

Discarding work forfeits any completed reads. Do not discard an in-progress drive
scan casually, and never remove `.keeper/catalog` or unlink `LOCK` to reset it.
See [checkpoint recovery](resume.md#work-state-and-recovery).

## Comparing policies

Matching policies permit normal comparison of selected paths. Reports still
explain that excluded paths are outside the check.

Different policies—including “no file” versus “empty file,” or comment-only
changes—produce a prominent **SCOPE MISMATCH**, diagnostic differences, and exit
code **2**, not a successful equality claim. Reported one-sided paths are observed
within independently selected scopes, not proof of absence outside them.
Catalog-only comparison continues to compare saved policies even if current files
have changed. `catalog check` validates the saved policy and checksums, not the
current configuration.
