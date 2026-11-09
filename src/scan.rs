use crate::{
    catalog::{Catalog, Entry},
    encoding,
    policy::Policy,
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
    pub policy: Policy,
    pub excluded: Vec<Vec<u8>>,
}

impl Inventory {
    pub fn file_sizes(&self) -> BTreeMap<Vec<u8>, u64> {
        self.files
            .iter()
            .map(|(path, meta)| (path.clone(), meta.len()))
            .collect()
    }

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
    let mut inventory = Inventory {
        policy: Policy::load(root)?,
        ..Inventory::default()
    };
    walk(root, root, &mut inventory, progress)?;
    inventory.policy.ensure_current(root)?;
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
        } else if relative == b".keeperignore" {
            // Root configuration is represented by the recorded policy, not data.
        } else if inventory.policy.excludes(&relative, meta.is_dir()) {
            inventory.excluded.push(relative);
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

/// Hash precisely the preflight inventory, checking directory identities before
/// and after. This is race detection, not a substitute for a read-only snapshot.
pub fn hash_inventory(
    root: &Path,
    inventory: Inventory,
    progress: &mut Progress<'_>,
) -> Result<Catalog> {
    hash_inventory_checkpointed(root, inventory, progress, None)
}

pub fn hash_inventory_checkpointed(
    root: &Path,
    inventory: Inventory,
    progress: &mut Progress<'_>,
    mut checkpoint: Option<&mut crate::checkpoint::Checkpoint>,
) -> Result<Catalog> {
    inventory.ensure_clean()?;
    inventory.policy.ensure_current(root)?;
    if let Some(work) = &checkpoint {
        ensure!(
            work.policy == inventory.policy,
            ".keeperignore differs from checkpoint scope; discard only the work file and restart"
        );
    }
    check_directories(root, &inventory)?;
    check_namespace(root, &inventory, progress)?;
    progress.begin("Hashing", Some((inventory.file_count(), inventory.bytes)))?;
    let mut catalog = Catalog::new();
    catalog.policy = inventory.policy.clone();
    for (relative, meta) in &inventory.files {
        crate::cancel::check()?;
        let path = joined(root, relative);
        let reused = match checkpoint.as_mut() {
            Some(work) => work.reuse(relative, meta)?,
            None => None,
        };
        let entry = match reused {
            Some(entry) => entry,
            None => {
                progress.begin_file(meta.len());
                let entry = hash_file(&path, meta, |n| {
                    progress.advance(0, n)?;
                    Ok(())
                })
                .with_context(|| format!("hash {}", label(relative)))?;
                progress.end_file();
                if let Some(work) = checkpoint.as_mut() {
                    work.record(relative, meta, &entry)?;
                }
                entry
            }
        };
        progress.advance(1, 0)?;
        catalog.insert(relative.clone(), entry);
    }
    progress.finish()?;
    check_directories(root, &inventory)?;
    check_namespace(root, &inventory, progress)?;
    // In particular, reused observations must still refer to this physical path.
    for (relative, before) in &inventory.files {
        crate::cancel::check()?;
        ensure!(
            same_file(before, &fs::symlink_metadata(joined(root, relative))?),
            "file {} changed during pass; stop writers and retry",
            label(relative)
        );
    }
    crate::cancel::check()?;
    inventory.policy.ensure_current(root)?;
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

/// Re-enumerate selected membership independently of directory timestamps.
/// Stream entries and look them up in the inventory: O(n log n), without a
/// second full in-memory inventory. Still not an atomic namespace snapshot.
fn check_namespace(root: &Path, inventory: &Inventory, progress: &mut Progress<'_>) -> Result<()> {
    progress.begin("Recheck paths", None)?;
    let mut files = 0usize;
    let mut directories = 1usize; // the root itself
    for relative_dir in inventory.directories.keys() {
        crate::cancel::check()?;
        for child in fs::read_dir(joined(root, relative_dir))? {
            progress.advance(0, 0)?; // cancellation, including inside huge directories
            let path = child?.path();
            let relative = relative(root, &path);
            let meta = fs::symlink_metadata(&path)?;
            if relative == b".keeper" {
                ensure!(meta.is_dir(), "reserved .keeper changed during pass");
                continue;
            }
            if relative == b".keeperignore" {
                continue;
            }
            if inventory.policy.excludes(&relative, meta.is_dir()) {
                continue;
            }
            if meta.is_file() {
                progress.advance(1, meta.len())?;
            }
            let known = if meta.is_dir() {
                directories += 1;
                inventory.directories.contains_key(&relative)
            } else if meta.is_file() {
                files += 1;
                inventory.files.contains_key(&relative)
            } else {
                false
            };
            ensure!(
                known,
                "namespace changed since preflight at {}; stop writers and retry",
                label(&relative)
            );
        }
    }
    ensure!(
        files == inventory.files.len() && directories == inventory.directories.len(),
        "namespace changed since preflight (missing selected paths); stop writers and retry"
    );
    progress.finish()?;
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
    fn policy_changes_after_preflight_abort_before_hashing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("data"), b"a").unwrap();
        fs::write(tmp.path().join(".keeperignore"), b"# original\n").unwrap();
        let mut sink = Vec::new();
        let mut progress = Progress::new(&mut sink, false);
        let inventory = preflight(tmp.path(), &mut progress).unwrap();
        fs::write(tmp.path().join(".keeperignore"), b"/data\n").unwrap();
        assert!(
            hash_inventory(tmp.path(), inventory, &mut progress)
                .unwrap_err()
                .to_string()
                .contains("changed during")
        );
    }

    #[test]
    fn scoped_checkpoints_reuse_only_with_the_same_policy() {
        use crate::{catalog::InitGuard, checkpoint::Checkpoint};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join(".keeperignore"), b"/skip/\n").unwrap();
        fs::write(root.join("data"), b"original").unwrap();
        let _guard = InitGuard::acquire(root).unwrap();
        let mut work = Checkpoint::open(root, "init", None).unwrap();
        let meta = fs::metadata(root.join("data")).unwrap();
        let entry = hash_file(&root.join("data"), &meta, |_| Ok(())).unwrap();
        work.record(b"data", &meta, &entry).unwrap();
        drop(work);
        let mut work = Checkpoint::open(root, "init", None).unwrap();
        let mut sink = Vec::new();
        let mut progress = Progress::new(&mut sink, false);
        let inventory = preflight(root, &mut progress).unwrap();
        let catalog =
            hash_inventory_checkpointed(root, inventory, &mut progress, Some(&mut work)).unwrap();
        assert_eq!(
            work.reused,
            u64::from(crate::filesystem::checkpoint_reuse_allowed(&root.join("data")).unwrap())
        );
        assert_eq!(catalog.policy, Policy::load(root).unwrap());
        // Even an already-open journal cannot be used after scope changes.
        fs::write(root.join(".keeperignore"), b"/data\n").unwrap();
        let inventory = preflight(root, &mut progress).unwrap();
        assert!(
            hash_inventory_checkpointed(root, inventory, &mut progress, Some(&mut work))
                .unwrap_err()
                .to_string()
                .contains("checkpoint scope")
        );
    }

    #[test]
    fn resumed_pass_reuses_per_path_and_rechecks_restored_mtime_changes() {
        use crate::{catalog::InitGuard, checkpoint::Checkpoint};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let _guard = InitGuard::acquire(root).unwrap();
        for name in ["a", "b", "duplicate"] {
            fs::write(root.join(name), b"original").unwrap();
        }
        // Simulate completed reads before an interrupted session. In particular,
        // knowing a's object hash must not skip duplicate's physical read.
        let mut work = Checkpoint::open(root, "init", None).unwrap();
        let session = work.session.clone();
        for name in ["a", "b"] {
            let path = root.join(name);
            let meta = fs::metadata(&path).unwrap();
            let entry = hash_file(&path, &meta, |_| Ok(())).unwrap();
            work.record(name.as_bytes(), &meta, &entry).unwrap();
        }
        drop(work);
        let path = root.join("b");
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, b"modified").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
        let mut work = Checkpoint::open(root, "init", None).unwrap();
        assert_eq!(work.session, session);
        let mut sink = Vec::new();
        let mut progress = Progress::new(&mut sink, false);
        let inventory = preflight(root, &mut progress).unwrap();
        let observed =
            hash_inventory_checkpointed(root, inventory, &mut progress, Some(&mut work)).unwrap();
        assert_eq!(
            work.reused,
            u64::from(crate::filesystem::checkpoint_reuse_allowed(&root.join("b")).unwrap())
        );
        assert_eq!(
            observed[b"b".as_slice()].hash,
            format!("b3:{}", blake3::hash(b"modified").to_hex())
        );
        assert_eq!(observed[b"a".as_slice()], observed[b"duplicate".as_slice()]);
        // Resume rediscovers the current namespace rather than blindly trusting
        // old paths, even if completed work included a now-missing path.
        fs::remove_file(root.join("a")).unwrap();
        fs::write(root.join("new"), b"new").unwrap();
        drop(work);
        let mut work = Checkpoint::open(root, "init", None).unwrap();
        let inventory = preflight(root, &mut progress).unwrap();
        let observed =
            hash_inventory_checkpointed(root, inventory, &mut progress, Some(&mut work)).unwrap();
        assert!(!observed.contains_key(b"a".as_slice()));
        assert!(observed.contains_key(b"new".as_slice()));
        assert_eq!(
            work.reused,
            2 * u64::from(crate::filesystem::checkpoint_reuse_allowed(&root.join("b")).unwrap())
        );
    }

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
    fn detects_metadata_visible_mutation_during_read() {
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
                // Force a visible timestamp difference without sleeps. A
                // restored mtime + colliding ctime is NOT detectable by stat;
                // checkpoint policy has a deterministic regression for that.
                file.set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))?;
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
    fn namespace_checks_do_not_trust_unchanged_directory_fingerprints() {
        for action in ["add", "remove", "rename", "type"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            fs::write(root.join("a"), b"a").unwrap();
            let mut sink = Vec::new();
            let mut progress = Progress::new(&mut sink, false);
            let mut inventory = preflight(root, &mut progress).unwrap();
            match action {
                "add" => fs::write(root.join("b"), b"b").unwrap(),
                "remove" => fs::remove_file(root.join("a")).unwrap(),
                "rename" => fs::rename(root.join("a"), root.join("b")).unwrap(),
                "type" => {
                    fs::remove_file(root.join("a")).unwrap();
                    fs::create_dir(root.join("a")).unwrap();
                }
                _ => unreachable!(),
            }
            // Make the saved directory identity equal to today's stat, modeling
            // a coarse-timestamp collision on every filesystem, without sleeps.
            inventory
                .directories
                .insert(Vec::new(), fs::symlink_metadata(root).unwrap());
            check_directories(root, &inventory).unwrap();
            assert!(
                check_namespace(root, &inventory, &mut progress).is_err(),
                "{action}"
            );
        }
    }

    #[test]
    fn namespace_recheck_preserves_exact_exclusion_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join(".keeperignore"), b"/ignored/\n").unwrap();
        fs::create_dir(root.join("ignored")).unwrap();
        fs::create_dir(root.join("empty")).unwrap();
        let mut sink = Vec::new();
        let mut progress = Progress::new(&mut sink, false);
        let inventory = preflight(root, &mut progress).unwrap();
        fs::write(root.join("ignored/new"), b"excluded").unwrap();
        check_namespace(root, &inventory, &mut progress).unwrap();
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
