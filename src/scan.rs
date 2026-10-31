use crate::{
    catalog::{Catalog, Entry},
    encoding,
    progress::Progress,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata, OpenOptions},
    io::Read,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Inventory {
    files: BTreeMap<Vec<u8>, Metadata>,
    directories: BTreeMap<Vec<u8>, Metadata>,
    pub bytes: u64,
    pub issues: Vec<String>,
}

impl Inventory {
    pub fn file_count(&self) -> u64 {
        self.files.len() as u64
    }

    pub fn ensure_clean(&self) -> Result<()> {
        ensure!(
            self.issues.is_empty(),
            "preflight found {} issue(s); file contents were not read:\n{}",
            self.issues.len(),
            self.issues.join("\n")
        );
        Ok(())
    }
}

/// Metadata-only discovery, accumulating issues instead of aborting at the first.
/// File readability and filesystem durability capabilities are NOT certified.
pub fn preflight(root: &Path, progress: &mut Progress<'_>) -> Result<Inventory> {
    progress.begin("Preflight", None)?;
    let mut inventory = Inventory::default();
    walk(root, root, &mut inventory, progress)?;
    progress.finish()?;
    Ok(inventory)
}

fn relative(root: &Path, path: &Path) -> Vec<u8> {
    path.strip_prefix(root)
        .expect("walk stays below root")
        .as_os_str()
        .as_bytes()
        .to_vec()
}

fn label(path: &[u8]) -> String {
    if path.is_empty() {
        ".".into()
    } else {
        encoding::encode(path)
    }
}

fn issue(inventory: &mut Inventory, path: &[u8], message: impl std::fmt::Display) {
    inventory
        .issues
        .push(format!("ISSUE\t{}\t{message}", label(path)));
}

fn walk(
    root: &Path,
    dir: &Path,
    inventory: &mut Inventory,
    progress: &mut Progress<'_>,
) -> Result<()> {
    let dir_relative = relative(root, dir);
    let before = match fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => meta,
        Ok(_) => {
            issue(
                inventory,
                &dir_relative,
                "directory replaced during traversal",
            );
            return Ok(());
        }
        Err(error) => {
            issue(inventory, &dir_relative, error);
            return Ok(());
        }
    };
    let children = match fs::read_dir(dir) {
        Ok(children) => children,
        Err(error) => {
            issue(inventory, &dir_relative, error);
            return Ok(());
        }
    };
    // Avoid collecting/sorting DirEntry handles for huge directories. BTreeMaps
    // provide canonical ordering after discovery without retaining those handles.
    for child in children {
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                issue(inventory, &dir_relative, error);
                continue;
            }
        };
        let path = child.path();
        let relative = relative(root, &path);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) => {
                issue(inventory, &relative, error);
                continue;
            }
        };
        if relative == b".keeper" {
            if !meta.is_dir() {
                issue(
                    inventory,
                    &relative,
                    "reserved .keeper must be a real directory, not a file or symlink",
                );
            }
        } else if meta.is_dir() {
            walk(root, &path, inventory, progress)?;
        } else if meta.is_file() {
            inventory.bytes = inventory
                .bytes
                .checked_add(meta.len())
                .context("inventory byte count overflow")?;
            progress.advance(1, meta.len())?;
            inventory.files.insert(relative, meta);
        } else {
            issue(
                inventory,
                &relative,
                "unsupported filesystem object (only regular files and directories are supported; symlinks are not followed)",
            );
        }
        progress.advance(0, 0)?;
    }
    match fs::symlink_metadata(dir) {
        Ok(after) if same_file(&before, &after) => {}
        Ok(_) => issue(
            inventory,
            &dir_relative,
            "directory changed during preflight; stop writers and retry",
        ),
        Err(error) => issue(inventory, &dir_relative, error),
    }
    inventory.directories.insert(dir_relative, before);
    Ok(())
}

pub fn scan(root: &Path, progress: &mut Progress<'_>) -> Result<Catalog> {
    let inventory = preflight(root, progress)?;
    hash_inventory(root, inventory, progress)
}

/// Hash precisely the preflight inventory, checking directory identities before
/// and after. This is race detection, not a substitute for a read-only snapshot.
pub fn hash_inventory(
    root: &Path,
    inventory: Inventory,
    progress: &mut Progress<'_>,
) -> Result<Catalog> {
    inventory.ensure_clean()?;
    check_directories(root, &inventory)?;
    progress.begin("Hashing", Some((inventory.file_count(), inventory.bytes)))?;
    let mut catalog = Catalog::new();
    for (relative, meta) in &inventory.files {
        let path = joined(root, relative);
        let entry = hash_file(&path, meta, |n| {
            progress.advance(0, n)?;
            Ok(())
        })
        .with_context(|| format!("hash {}", label(relative)))?;
        progress.advance(1, 0)?;
        catalog.insert(relative.clone(), entry);
    }
    check_directories(root, &inventory)?;
    progress.finish()?;
    Ok(catalog)
}

fn joined(root: &Path, relative: &[u8]) -> PathBuf {
    root.join(std::ffi::OsString::from_vec(relative.to_vec()))
}

fn check_directories(root: &Path, inventory: &Inventory) -> Result<()> {
    for (relative, before) in &inventory.directories {
        let after = fs::symlink_metadata(joined(root, relative))
            .with_context(|| format!("recheck directory {}", label(relative)))?;
        ensure!(
            same_file(before, &after),
            "directory {} changed since preflight; stop writers and retry",
            label(relative)
        );
    }
    Ok(())
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.size() == b.size()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

fn hash_file(
    path: &Path,
    expected: &Metadata,
    on_read: impl FnMut(u64) -> Result<()>,
) -> Result<Entry> {
    // Nonblocking prevents a replacement FIFO from hanging open; NOFOLLOW
    // rejects a replacement symlink at the final path component.
    let mut file: File = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    ensure!(
        before.is_file() && same_file(expected, &before),
        "file replaced before hashing"
    );
    let entry = hash_stream(&mut file, before.size(), on_read)?;
    let after = file.metadata()?;
    let at_path = fs::symlink_metadata(path)?;
    ensure!(
        same_file(&before, &after) && same_file(&after, &at_path),
        "file changed while hashing; retry with writers stopped"
    );
    Ok(entry)
}

fn hash_stream(
    reader: &mut impl Read,
    expected_size: u64,
    mut on_read: impl FnMut(u64) -> Result<()>,
) -> Result<Entry> {
    let mut hash = blake3::Hasher::new();
    let mut buffer = [0u8; 128 * 1024];
    let mut size = 0u64;
    loop {
        let n = match reader.read(&mut buffer) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            break;
        }
        size += n as u64;
        ensure!(
            size <= expected_size,
            "file grew while hashing; retry with writers stopped"
        );
        hash.update(&buffer[..n]);
        on_read(n as u64)?;
    }
    ensure!(
        size == expected_size,
        "file shrank while hashing; retry with writers stopped"
    );
    Ok(Entry {
        hash: format!("b3:{}", hash.finalize().to_hex()),
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_files_larger_than_the_read_buffer() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("large");
        let bytes: Vec<u8> = (0..400_001).map(|n| (n % 251) as u8).collect();
        fs::write(&path, &bytes).unwrap();
        let entry = hash_file(&path, &fs::symlink_metadata(&path).unwrap(), |_| Ok(())).unwrap();
        assert_eq!(entry.hash, format!("b3:{}", blake3::hash(&bytes).to_hex()));
        assert_eq!(entry.size, bytes.len() as u64);
    }

    #[test]
    fn refuses_a_file_replaced_after_traversal_stat() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file");
        fs::write(&path, b"original").unwrap();
        let expected = fs::symlink_metadata(&path).unwrap();
        let replacement = tmp.path().join("replacement");
        fs::write(&replacement, b"replaced").unwrap();
        fs::rename(replacement, &path).unwrap();
        assert!(hash_file(&path, &expected, |_| Ok(())).is_err());
    }

    #[test]
    fn refuses_a_symlink_swapped_in_after_stat() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file");
        fs::write(&path, b"original").unwrap();
        let expected = fs::symlink_metadata(&path).unwrap();
        fs::rename(&path, tmp.path().join("target")).unwrap();
        std::os::unix::fs::symlink("target", &path).unwrap();
        assert!(hash_file(&path, &expected, |_| Ok(())).is_err());
    }

    #[test]
    fn detects_mutation_during_read_even_with_restored_mtime() {
        use std::io::{Seek, SeekFrom, Write};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file");
        fs::write(&path, vec![0; 400_001]).unwrap();
        let expected = fs::symlink_metadata(&path).unwrap();
        let mut changed = false;
        let result = hash_file(&path, &expected, |_| {
            if !changed {
                let mut file = OpenOptions::new().write(true).open(&path)?;
                file.seek(SeekFrom::Start(200_000))?;
                file.write_all(b"changed")?;
                // Deliberately change mtime, then restore it. ctime cannot be
                // restored through this API, so metadata must still catch it.
                file.set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))?;
                file.set_times(fs::FileTimes::new().set_modified(expected.modified()?))?;
                changed = true;
            }
            Ok(())
        });
        assert!(result.is_err());
    }

    #[test]
    fn catches_namespace_changes_after_preflight() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a"), b"abc").unwrap();
        let mut sink = Vec::new();
        let mut progress = Progress::new(&mut sink, false);
        let inventory = preflight(tmp.path(), &mut progress).unwrap();
        fs::write(tmp.path().join("new"), b"new").unwrap();
        assert!(hash_inventory(tmp.path(), inventory, &mut progress).is_err());
    }

    #[test]
    fn stream_rejects_short_long_and_failed_reads_and_retries_interrupts() {
        use std::io::{self, Cursor};
        assert!(hash_stream(&mut Cursor::new(b"abc"), 4, |_| Ok(())).is_err());
        assert!(hash_stream(&mut Cursor::new(b"abc"), 2, |_| Ok(())).is_err());
        struct InterruptedOnce(bool);
        impl Read for InterruptedOnce {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(0)
                }
            }
        }
        assert!(hash_stream(&mut InterruptedOnce(false), 0, |_| Ok(())).is_ok());
        struct FailsAfterData(bool);
        impl Read for FailsAfterData {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    buffer[0] = b'a';
                    Ok(1)
                } else {
                    Err(io::Error::other("injected I/O error"))
                }
            }
        }
        assert!(hash_stream(&mut FailsAfterData(false), 1, |_| Ok(())).is_err());
        // Official BLAKE3 test vector; not merely a second use of our own loop.
        assert_eq!(
            hash_stream(&mut Cursor::new(b"abc"), 3, |_| Ok(()))
                .unwrap()
                .hash,
            "b3:6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }
}
