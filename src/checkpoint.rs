//! Disposable per-path observations, never an accepted baseline or fresh scrub.
use crate::{
    catalog::{self, Catalog, Entry},
    encoding,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata, OpenOptions},
    io::Write,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn fingerprint(meta: &Metadata) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        meta.dev(),
        meta.ino(),
        meta.size(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec()
    )
}

fn now() -> Result<u128> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())
}

fn checked(payload: &str) -> String {
    format!("{payload}\t{}\n", blake3::hash(payload.as_bytes()).to_hex())
}

fn verify(line: &str) -> Result<&str> {
    let (payload, checksum) = line
        .rsplit_once('\t')
        .context("invalid checkpoint record")?;
    ensure!(
        blake3::hash(payload.as_bytes()).to_hex().as_str() == checksum,
        "checkpoint checksum mismatch; discard work file and restart"
    );
    Ok(payload)
}

pub struct Checkpoint {
    path: PathBuf,
    file: File,
    records: BTreeMap<Vec<u8>, (String, Entry)>,
    pub session: String,
    pub reused: u64,
}

impl Checkpoint {
    /// Caller holds the dataset writer lock for this object's entire lifetime.
    pub fn open(root: &Path, operation: &str, baseline: Option<&Catalog>) -> Result<Self> {
        let dir = catalog::metadata_dir(root)?;
        let path = dir.join(format!("{operation}.work"));
        let meta = fs::metadata(root)?;
        let baseline = match baseline {
            Some(catalog) => {
                let (paths, objects) = catalog::tables(catalog)?;
                format!(
                    "{}:{}",
                    blake3::hash(paths.as_bytes()).to_hex(),
                    blake3::hash(objects.as_bytes()).to_hex()
                )
            }
            None => "genesis".into(),
        };
        let identity = format!(
            "keeper-checkpoint\t1\t{operation}\t{}\t{}\t{}\t{baseline}",
            encoding::encode(root.as_os_str().as_bytes()),
            meta.dev(),
            meta.ino()
        );
        // Publish even the initial header atomically: a crash cannot leave an
        // apparently valid session with a partially written identity.
        if fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
            let mut temp = tempfile::Builder::new()
                .prefix(".work-")
                .tempfile_in(&dir)?;
            let session = format!(
                "{}-{}",
                now()?,
                temp.path().file_name().unwrap().to_string_lossy()
            );
            temp.write_all(checked(&format!("{identity}\t{session}")).as_bytes())?;
            temp.as_file().sync_all()?;
            catalog::rename_no_replace(temp.path(), &path)?;
            File::open(&dir)?.sync_all()?;
        }
        let bytes = catalog::read_regular(&path)?;
        // A killed append can leave a partial last line. Only that suffix may
        // be dropped; corruption of a complete record is an operational error.
        let end = bytes
            .iter()
            .rposition(|&b| b == b'\n')
            .context("incomplete checkpoint header")?
            + 1;
        let text = std::str::from_utf8(&bytes[..end])?;
        let mut lines = text.lines();
        let header = verify(lines.next().context("missing checkpoint header")?)?;
        let (found, session) = header
            .rsplit_once('\t')
            .context("missing session identity")?;
        ensure!(
            found == identity,
            "checkpoint version, operation, root identity or baseline mismatch; discard {} and restart",
            path.display()
        );
        ensure!(!session.is_empty(), "empty checkpoint session");
        let mut records = BTreeMap::new();
        for line in lines {
            let fields: Vec<_> = verify(line)?.split('\t').collect();
            ensure!(fields.len() == 5, "invalid checkpoint fields");
            let relative = encoding::decode(fields[0])?;
            encoding::validate_path(&relative)?;
            ensure!(
                encoding::encode(&relative) == fields[0],
                "noncanonical checkpoint path"
            );
            ensure!(catalog::valid_hash(fields[2]), "invalid checkpoint hash");
            let size: u64 = fields[3].parse()?;
            let _: u128 = fields[4].parse()?;
            ensure!(size.to_string() == fields[3], "invalid checkpoint size");
            records.insert(
                relative,
                (
                    fields[1].into(),
                    Entry {
                        hash: fields[2].into(),
                        size,
                    },
                ),
            );
        }
        let file = OpenOptions::new()
            .append(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        ensure!(
            file.metadata()?.is_file(),
            "checkpoint must be a regular file"
        );
        file.set_len(end as u64)?;
        file.sync_all()?;
        Ok(Self {
            path,
            file,
            records,
            session: session.into(),
            reused: 0,
        })
    }

    pub fn reuse(&mut self, path: &[u8], meta: &Metadata) -> Option<Entry> {
        let (identity, entry) = self.records.get(path)?;
        if *identity == fingerprint(meta) && entry.size == meta.size() {
            self.reused += 1;
            Some(entry.clone())
        } else {
            None
        }
    }

    pub fn record(&mut self, path: &[u8], meta: &Metadata, entry: &Entry) -> Result<()> {
        let payload = format!(
            "{}\t{}\t{}\t{}\t{}",
            encoding::encode(path),
            fingerprint(meta),
            entry.hash,
            entry.size,
            now()?
        );
        self.file.write_all(checked(&payload).as_bytes())?;
        // Per-file sync bounds lost work to the current file, without rewriting
        // an ever-growing inventory on every completion.
        self.file.sync_all()?;
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        fs::remove_file(&self.path)?;
        File::open(self.path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}
