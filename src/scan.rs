use crate::{
    catalog::{Catalog, Entry},
    encoding,
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

pub fn scan(root: &Path) -> Result<Catalog> {
    let mut catalog = Catalog::new();
    walk(root, root, &mut catalog)?;
    Ok(catalog)
}

fn walk(root: &Path, dir: &Path, catalog: &mut Catalog) -> Result<()> {
    let before = fs::symlink_metadata(dir)?;
    ensure!(before.is_dir(), "directory replaced during traversal");
    let mut children: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let path = child.path();
        let relative = path.strip_prefix(root)?.as_os_str().as_bytes();
        if relative == b".keeper" {
            continue;
        }
        let label = encoding::encode(relative);
        let meta = fs::symlink_metadata(&path).with_context(|| format!("stat {label}"))?;
        if meta.is_dir() {
            walk(root, &path, catalog).with_context(|| format!("scan {label}"))?;
        } else if meta.is_file() {
            let entry = hash_file(&path, &meta).with_context(|| format!("hash {label}"))?;
            catalog.insert(relative.to_vec(), entry);
        } else {
            bail!(
                "unsupported filesystem object: {label} (only regular files and directories are supported; symlinks are not followed)"
            );
        }
    }
    let after = fs::symlink_metadata(dir)?;
    ensure!(
        same_file(&before, &after),
        "directory changed during traversal; retry with writers stopped"
    );
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

fn hash_file(path: &Path, expected: &Metadata) -> Result<Entry> {
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
    let mut hash = blake3::Hasher::new();
    let mut buffer = [0u8; 128 * 1024];
    let mut size = 0u64;
    loop {
        let n = match file.read(&mut buffer) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            break;
        }
        size += n as u64;
        ensure!(
            size <= before.size(),
            "file grew while hashing; retry with writers stopped"
        );
        hash.update(&buffer[..n]);
    }
    let after = file.metadata()?;
    let at_path = fs::symlink_metadata(path)?;
    ensure!(
        size == before.size() && same_file(&before, &after) && same_file(&after, &at_path),
        "file changed while hashing; retry with writers stopped"
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
        let entry = hash_file(&path, &fs::symlink_metadata(&path).unwrap()).unwrap();
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
        assert!(hash_file(&path, &expected).is_err());
    }

    #[test]
    fn refuses_a_symlink_swapped_in_after_stat() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file");
        fs::write(&path, b"original").unwrap();
        let expected = fs::symlink_metadata(&path).unwrap();
        fs::rename(&path, tmp.path().join("target")).unwrap();
        std::os::unix::fs::symlink("target", &path).unwrap();
        assert!(hash_file(&path, &expected).is_err());
    }
}
