use crate::{encoding, policy::Policy};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    files: BTreeMap<Vec<u8>, Entry>,
    pub policy: Policy,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::ops::Deref for Catalog {
    type Target = BTreeMap<Vec<u8>, Entry>;
    fn deref(&self) -> &Self::Target {
        &self.files
    }
}
impl std::ops::DerefMut for Catalog {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.files
    }
}
impl<'a> IntoIterator for &'a Catalog {
    type Item = (&'a Vec<u8>, &'a Entry);
    type IntoIter = std::collections::btree_map::Iter<'a, Vec<u8>, Entry>;
    fn into_iter(self) -> Self::IntoIter {
        self.files.iter()
    }
}
const PATHS_HEADER: &str = "path\tkind\tobject\tsize\n";
const OBJECTS_HEADER: &str = "object\tsize\n";
const MANIFEST_HEADER: &str = "keeper-catalog-version\t1\ngeneration\t1\n";

fn manifest(objects: &[u8], paths: &[u8], policy: &Policy) -> String {
    match &policy.source {
        None => format!(
            "{MANIFEST_HEADER}file\tobjects.tsv\t{}\nfile\tpaths.tsv\t{}\n",
            digest(objects),
            digest(paths)
        ),
        Some(source) => format!(
            "keeper-catalog-version\t2\ngeneration\t1\nscope-policy\tkeeperignore-1\nfile\tobjects.tsv\t{}\nfile\tpaths.tsv\t{}\nfile\tscope.ignore\t{}\n",
            digest(objects),
            digest(paths),
            digest(source)
        ),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

pub(crate) fn valid_hash(hash: &str) -> bool {
    hash.len() == 67
        && hash.starts_with("b3:")
        && hash.as_bytes()[3..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

fn number(text: &str) -> Result<u64> {
    let value: u64 = text.parse()?;
    ensure!(value.to_string() == text, "noncanonical integer");
    Ok(value)
}

fn objects(catalog: &Catalog) -> Result<BTreeMap<&str, u64>> {
    let mut objects = BTreeMap::new();
    for entry in catalog.values() {
        if let Some(size) = objects.insert(entry.hash.as_str(), entry.size) {
            ensure!(size == entry.size, "same hash has conflicting sizes");
        }
    }
    Ok(objects)
}

pub(crate) fn tables(catalog: &Catalog) -> Result<(String, String)> {
    let mut paths = PATHS_HEADER.to_owned();
    for (path, entry) in catalog {
        encoding::validate_path(path)?;
        if catalog.policy.source.is_some() {
            ensure!(
                path != b".keeperignore" && !catalog.policy.excludes(path, false),
                "catalog contains a path excluded by its recorded scope policy"
            );
        }
        for (index, &byte) in path.iter().enumerate() {
            if byte == b'/' {
                ensure!(
                    !catalog.contains_key(&path[..index]),
                    "a catalog path cannot be both a file and a parent directory"
                );
            }
        }
        ensure!(valid_hash(&entry.hash), "invalid BLAKE3 object ID");
        paths.push_str(&format!(
            "{}\tfile\t{}\t{}\n",
            encoding::encode(path),
            entry.hash,
            entry.size
        ));
    }
    let mut object_table = OBJECTS_HEADER.to_owned();
    for (hash, size) in objects(catalog)? {
        object_table.push_str(&format!("{hash}\t{size}\n"));
    }
    Ok((paths, object_table))
}

pub(crate) fn metadata_dir(root: &Path) -> Result<PathBuf> {
    let dir = root.join(".keeper");
    let meta = fs::symlink_metadata(&dir).context("read .keeper (run keeper init first)")?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        ".keeper must be a real directory"
    );
    Ok(dir)
}

/// Writer exclusion for init or checkpointed status. Hold for the whole operation.
/// Kernel locks are released automatically after a crash.
pub struct InitGuard {
    dir: PathBuf,
    _lock: File,
}

impl InitGuard {
    #[cfg(test)]
    pub fn acquire(root: &Path) -> Result<Self> {
        let guard = Self::lock(root)?;
        ensure!(
            !guard.catalog_exists()?,
            "catalog already exists; init never overwrites a baseline"
        );
        Ok(guard)
    }

    pub fn catalog_exists(&self) -> Result<bool> {
        match fs::symlink_metadata(self.dir.join("catalog")) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("cannot inspect .keeper/catalog"),
        }
    }

    pub fn lock(root: &Path) -> Result<Self> {
        let dir = root.join(".keeper");
        match fs::create_dir(&dir) {
            Ok(()) => File::open(root)?.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let dir = metadata_dir(root)?;
        use std::os::unix::fs::OpenOptionsExt;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(dir.join("LOCK"))?;
        ensure!(lock.metadata()?.is_file(), "LOCK must be a regular file");
        lock.try_lock_exclusive()
            .context("another Keeper writer holds the lock")?;
        File::open(&dir)?.sync_all()?;
        Ok(Self { dir, _lock: lock })
    }

    pub fn publish(&self, catalog: &Catalog) -> Result<()> {
        let (paths, objects) = tables(catalog)?;
        let manifest = manifest(objects.as_bytes(), paths.as_bytes(), &catalog.policy);

        let staging = tempfile::Builder::new()
            .prefix(".init-")
            .tempdir_in(&self.dir)?;
        for (name, contents) in [
            ("paths.tsv", paths),
            ("objects.tsv", objects),
            ("MANIFEST", manifest),
        ] {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staging.path().join(name))?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
        }
        if let Some(source) = &catalog.policy.source {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staging.path().join("scope.ignore"))?;
            file.write_all(source)?;
            file.sync_all()?;
        }
        // Use the production reader before making a generation authoritative.
        ensure!(
            read_generation(staging.path())? == *catalog,
            "catalog round-trip failed"
        );
        File::open(staging.path())?.sync_all()?;
        // Linux RENAME_NOREPLACE: even an unrelated writer cannot cause init to
        // replace an existing (including empty) catalog directory.
        rename_no_replace(staging.path(), &self.dir.join("catalog"))?;
        File::open(&self.dir)?
            .sync_all()
            .context("catalog published, but directory fsync failed; inspect with catalog check")?;
        Ok(())
    }
}

pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both C strings live through the call; no raw memory is written.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("publish catalog without replacement");
    }
    Ok(())
}

pub fn load(root: &Path) -> Result<Catalog> {
    read_generation(&metadata_dir(root)?.join("catalog"))
        .context("catalog is invalid; refusing to use it as a baseline")
}

pub(crate) fn read_regular(path: &Path) -> Result<Vec<u8>> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "catalog members must be regular files"
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_generation(dir: &Path) -> Result<Catalog> {
    ensure!(
        fs::symlink_metadata(dir)?.is_dir(),
        "catalog must be a real directory"
    );
    let mut names: Vec<_> = fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()?;
    names.sort();
    let scoped = names == ["MANIFEST", "objects.tsv", "paths.tsv", "scope.ignore"];
    ensure!(
        scoped || names == ["MANIFEST", "objects.tsv", "paths.tsv"],
        "unexpected catalog file set"
    );
    let policy = if scoped {
        Policy::parse(read_regular(&dir.join("scope.ignore"))?)?
    } else {
        Policy::default()
    };
    let manifest = String::from_utf8(read_regular(&dir.join("MANIFEST"))?)?;
    let objects_bytes = read_regular(&dir.join("objects.tsv"))?;
    let paths_bytes = read_regular(&dir.join("paths.tsv"))?;
    let expected_manifest = self::manifest(&objects_bytes, &paths_bytes, &policy);
    ensure!(
        manifest == expected_manifest,
        "unsupported manifest or catalog checksum mismatch"
    );
    let paths = std::str::from_utf8(&paths_bytes)?;
    let records = paths
        .strip_prefix(PATHS_HEADER)
        .context("invalid paths header")?;
    ensure!(
        records.is_empty() || records.ends_with('\n'),
        "unterminated paths record"
    );
    let mut catalog = Catalog {
        policy,
        ..Catalog::new()
    };
    for line in records.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        ensure!(
            fields.len() == 4 && fields[1] == "file",
            "invalid paths record"
        );
        let path = encoding::decode(fields[0])?;
        encoding::validate_path(&path)?;
        ensure!(valid_hash(fields[2]), "invalid object ID");
        if catalog
            .insert(
                path,
                Entry {
                    hash: fields[2].to_owned(),
                    size: number(fields[3])?,
                },
            )
            .is_some()
        {
            bail!("duplicate path");
        }
    }
    let (canonical_paths, canonical_objects) = tables(&catalog)?;
    ensure!(
        canonical_paths.as_bytes() == paths_bytes,
        "noncanonical paths table"
    );
    ensure!(
        canonical_objects.as_bytes() == objects_bytes,
        "invalid objects table or inconsistent object references"
    );
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_never_replaces_even_an_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = InitGuard::acquire(tmp.path()).unwrap();
        fs::create_dir(tmp.path().join(".keeper/catalog")).unwrap();
        assert!(guard.publish(&Catalog::new()).is_err());
        assert_eq!(
            fs::read_dir(tmp.path().join(".keeper/catalog"))
                .unwrap()
                .count(),
            0
        );
    }

    // Subprocess-only test helper. No fault-injection switches exist in the
    // production binary. The parent kills us while holding the real writer lock.
    #[test]
    fn crash_helper() {
        let Some(root) = std::env::var_os("KEEPER_TEST_CRASH_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let guard = InitGuard::acquire(&root).unwrap();
        if std::env::var("KEEPER_TEST_CRASH_STAGE").unwrap() == "after" {
            guard.publish(&Catalog::new()).unwrap();
        } else {
            let staging = tempfile::Builder::new()
                .prefix(".init-")
                .tempdir_in(&guard.dir)
                .unwrap();
            fs::write(staging.path().join("paths.tsv"), b"incomplete").unwrap();
            // Deliberately leak ownership: SIGKILL must not rely on destructors.
            std::mem::forget(staging);
        }
        println!("CRASH_TEST_READY");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    #[test]
    fn process_death_before_and_after_publication_leaves_no_partial_catalog() {
        use std::{
            io::{BufRead, BufReader},
            process::{Command, Stdio},
        };
        struct KillOnDrop(std::process::Child);
        impl Drop for KillOnDrop {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        for stage in ["before", "after"] {
            let tmp = tempfile::tempdir().unwrap();
            let mut child = KillOnDrop(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "catalog::tests::crash_helper", "--nocapture"])
                    .env("KEEPER_TEST_CRASH_ROOT", tmp.path())
                    .env("KEEPER_TEST_CRASH_STAGE", stage)
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            let reader = BufReader::new(child.0.stdout.take().unwrap());
            let ready = reader
                .lines()
                .any(|line| line.unwrap().contains("CRASH_TEST_READY"));
            assert!(ready, "child failed before reaching crash point");
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            if stage == "before" {
                assert!(!tmp.path().join(".keeper/catalog").exists());
                let guard = InitGuard::acquire(tmp.path()).unwrap();
                guard.publish(&Catalog::new()).unwrap();
            }
            assert_eq!(load(tmp.path()).unwrap(), Catalog::new());
            assert!(InitGuard::acquire(tmp.path()).is_err()); // never rebaseline
        }
    }

    #[test]
    fn writer_lock_is_exclusive() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = InitGuard::acquire(tmp.path()).unwrap();
        assert!(InitGuard::acquire(tmp.path()).is_err());
        drop(guard);
        assert!(InitGuard::acquire(tmp.path()).is_ok());
    }
}
