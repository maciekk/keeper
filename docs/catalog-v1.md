# Catalog format 1: genesis catalog

This document describes the implemented format, not just a proposed schema.
Future incompatible schema changes must use a new format version; do not silently
reinterpret catalogs already written by this implementation.

```text
.keeper/
├── LOCK                    # advisory writer lock; not authoritative
├── .init-*/                # temporary staging, possibly left after a crash
└── catalog/                # complete authoritative state
    ├── MANIFEST
    ├── objects.tsv
    └── paths.tsv
```

The reader requires exactly these three regular files in `catalog`, with no
symlinks or extra members. This initial implementation supports only genesis
(`generation 1`). It has no catalog-update operation.

## Text rules

All files contain ASCII, use LF line endings, and end in LF. Fields are separated
by literal tabs, including in the headers. No comments, blank lines, or optional
columns. Unsigned decimal integers have no sign or leading zeros (except `0`).
Hashes have the form `b3:` followed by **64 lowercase hexadecimal digits**: full
256-bit BLAKE3, no abbreviations.

Paths represent Unix filename **bytes**, not necessarily UTF-8. Encode bytes as:

| Byte | Representation |
| --- | --- |
| Backslash | `\\` |
| Tab | `\t` |
| LF | `\n` |
| CR | `\r` |
| Other printable ASCII (`0x20`–`0x7e`) | Literal byte |
| Everything else | `\xnn` (two lowercase hex digits) |

Encoding is canonical: `A` cannot be encoded as `\x41`; newline must be `\n`, not
`\x0a`. UTF-8 multibyte names are escaped byte by byte. A separator `/` remains
literal. Paths must be nonempty relative paths with no NUL bytes, empty components,
`.` or `..` components, or top-level `.keeper` component. A file path cannot also
be the ancestor of another file path.

## paths.tsv

Header:

```text
path<TAB>kind<TAB>object<TAB>size<LF>
```

Each record contains an escaped relative path, literal `file`, object hash, and
byte size. Paths are unique and sorted lexicographically by **decoded raw bytes**,
not by their escaped rendering. No mtime is authoritative in this milestone.

## objects.tsv

Header:

```text
object<TAB>size<LF>
```

One record per distinct hash referenced by `paths.tsv`, sorted lexicographically
by hash. Every path must reference an object of the same size; no unreferenced
objects. Multiple paths with identical bytes share one object record, but each
physical copy must still be hashed by a full check.

## MANIFEST

Exactly these four lines (replace `<...>` with actual full IDs):

```text
keeper-catalog-version<TAB>1
generation<TAB>1
file<TAB>objects.tsv<TAB>b3:<hash of exact objects.tsv bytes>
file<TAB>paths.tsv<TAB>b3:<hash of exact paths.tsv bytes>
```

The manifest hashes the complete table bytes, including headers, tabs and final
newlines. The manifest itself has no checksum or signature. `catalog check` first
validates the file set and checksums, then decodes paths, validates invariants,
and requires that reserialization produces exactly the original table bytes.

## Inspection without Keeper

`b3sum` can hash the data and the two catalog tables independently:

```sh
b3sum .keeper/catalog/objects.tsv .keeper/catalog/paths.tsv
b3sum -- photos/example.jpg

# List paths with their expected hashes:
awk -F '\t' 'NR > 1 { print $1, $3 }' .keeper/catalog/paths.tsv

# Find paths sharing a particular full object ID:
awk -F '\t' -v object='b3:REPLACE_WITH_FULL_HASH' \
  'NR > 1 && $3 == object { print $1 }' .keeper/catalog/paths.tsv
```

`b3sum` prints hex IDs without Keeper's `b3:` prefix. The path column contains
escaped names; don't pass it directly to a shell or blindly convert it into
commands. The example manual file check uses an ordinary literal filename.

## Genesis publication protocol

1. Create/check `.keeper` as a real directory and acquire an exclusive advisory
   lock on regular file `LOCK`. Refuse any existing catalog, even an empty one.
2. Scan/hash the dataset without changing it. Errors abort initialization.
3. Build all three catalog files in a same-directory `.init-*` staging directory.
4. Flush each file with `fsync`; read the generation back through the real reader
   and compare it to the observed catalog. Sync the staging directory.
5. Publish using Linux `renameat2(RENAME_NOREPLACE)`; no replacement is possible.
6. Sync `.keeper` before reporting success.

The root directory is also synced when `.keeper` is initially created. Directory
sync errors are errors, not ignored warnings. If the final directory sync fails,
the catalog may already be visible; inspect it rather than attempting to rebaseline.
A process crash before publication leaves no authoritative catalog. A crash during
publication exposes either no catalog or a complete generation, subject to filesystem
and storage guarantees. This is **not** yet a protocol for replacing generations;
that needs a separate design before `accept` is implemented.
