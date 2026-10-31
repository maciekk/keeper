# Catalog format 2: recorded exclusion policy

Format 2 extends [format 1](catalog-v1.md) only for newly initialized roots that
have a `.keeperignore` file. Unscoped roots still write byte-identical format 1;
both versions are readable. Existing catalogs are never rewritten or implicitly
migrated, and application upgrades do not require rehashing them.

## Files

```text
.keeper/catalog/
├── MANIFEST
├── objects.tsv
├── paths.tsv
└── scope.ignore
```

`objects.tsv` and `paths.tsv` have exactly the format-1 encoding, ordering, and
consistency rules. `scope.ignore` contains the exact original `.keeperignore`
bytes (including comments, whitespace, and line endings). It must parse under
[keeperignore-1](ignore.md#supported-syntax-keeperignore-1).

The canonical UTF-8/ASCII manifest is:

```text
keeper-catalog-version\t2
generation\t1
scope-policy\tkeeperignore-1
file\tobjects.tsv\tb3:OBJECTS_BLAKE3_HEX
file\tpaths.tsv\tb3:PATHS_BLAKE3_HEX
file\tscope.ignore\tb3:POLICY_BLAKE3_HEX
```

Here `\t` denotes a literal tab. Each line, including the last, ends in LF. Hashes have the
same full lowercase encoding as format 1. File rows are in the exact order shown.
There are exactly four regular-file members; symlinks, extra members, missing
members, noncanonical manifests, or mismatched checksums are rejected.

The reader also rejects paths that violate the recorded policy, including a data
entry for root `.keeperignore`. Nested `.keeperignore` paths are ordinary data
unless excluded by a rule. Directory-only exclusions are checked against ancestor
directory components of recorded regular-file paths.

## Publication and use

The existing exclusive-lock, staged-write, self-read/check, file/directory-sync,
and atomic no-replace [publication protocol](catalog-v1.md#genesis-publication-protocol)
now includes `scope.ignore` and its checksum. The policy is checked against current
configuration before publication. Neither format provides snapshot semantics.

Status compares the current policy with this saved source before hashing. Catalog
comparison compares saved sources, while live and metadata comparison compare
current sources. A policy mismatch returns exit 2; see [scope semantics](ignore.md).

Older Keeper builds that only understand format 1 reject format 2 rather than
ignoring its policy. Existing format-1 catalogs remain unchanged and usable; there
is currently no operation to change an established baseline's scope. Hashes and
path tables are retained without conversion, not discarded to introduce ignores.
Catalog checksums detect accidental damage, not coordinated malicious changes.
