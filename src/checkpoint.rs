//! Disposable per-path observations, never an accepted baseline or fresh scrub.
use crate::{
    catalog::{self, Catalog, Entry},
    encoding,
    policy::Policy,
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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

// Checked at file completion, not by a background timer.
const SYNC_INTERVAL: Duration = Duration::from_secs(5);

pub struct Checkpoint {
    path: PathBuf,
    file: File,
    records: BTreeMap<Vec<u8>, (String, Entry)>,
    sync_interval: Duration,
    last_sync: Instant,
    pub session: String,
    pub reused: u64,
    pub policy: Policy,
}

impl Checkpoint {
    /// Caller holds the dataset writer lock for this object's entire lifetime.
    pub fn open(root: &Path, operation: &str, baseline: Option<&Catalog>) -> Result<Self> {
        let dir = catalog::metadata_dir(root)?;
        let path = dir.join(format!("{operation}.work"));
        let meta = fs::metadata(root)?;
        let policy = Policy::load(root)?;
        if let Some(baseline) = baseline {
            ensure!(
                baseline.policy == policy,
                ".keeperignore differs from the catalog scope; status cannot change the baseline scope"
            );
        }
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
        let mut identity = format!(
            "keeper-checkpoint\t{}\t{operation}\t{}\t{}\t{}\t{baseline}",
            if policy.source.is_some() { 2 } else { 1 },
            encoding::encode(root.as_os_str().as_bytes()),
            meta.dev(),
            meta.ino()
        );
        if policy.source.is_some() {
            identity.push_str(&format!("\t{}", policy.id()));
        }
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
            "checkpoint version, operation, root identity, scope policy or baseline mismatch; discard {} and restart",
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
            sync_interval: SYNC_INTERVAL,
            last_sync: Instant::now(),
            session: session.into(),
            reused: 0,
            policy,
        })
    }

    pub fn with_sync_interval(mut self, interval: Duration) -> Self {
        self.sync_interval = interval;
        self
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
        // Append immediately so process cancellation/death retains complete rows
        // in the kernel cache. Batch durability flushes to avoid a read/flush/seek
        // cycle for every small file on spinning disks. A system crash can lose
        // the pending batch; missing observations are safely reread on resume.
        if self.last_sync.elapsed() >= self.sync_interval {
            self.file.sync_all()?;
            self.last_sync = Instant::now();
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        fs::remove_file(&self.path)?;
        File::open(self.path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::InitGuard;

    #[test]
    fn batches_sync_by_configured_interval_and_reopens_pending_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("data"), b"data").unwrap();
        let meta = fs::metadata(root.join("data")).unwrap();
        let entry = Entry {
            hash: format!("b3:{}", blake3::hash(b"data").to_hex()),
            size: 4,
        };
        let _guard = InitGuard::acquire(root).unwrap();
        let mut work = Checkpoint::open(root, "init", None)
            .unwrap()
            .with_sync_interval(Duration::from_secs(10));
        // Set synthetic timestamps rather than sleeping; no record-count trigger.
        // A future timestamp keeps slow CI from reaching the time threshold.
        let future = Instant::now() + Duration::from_secs(60);
        work.last_sync = future;
        for _ in 0..512 {
            work.record(b"data", &meta, &entry).unwrap();
        }
        assert_eq!(work.last_sync, future);

        // A longer configured interval must not flush at the default threshold.
        work.sync_interval = Duration::from_secs(3600);
        work.last_sync = Instant::now() - Duration::from_secs(60);
        let not_expired = work.last_sync;
        work.record(b"data", &meta, &entry).unwrap();
        assert_eq!(work.last_sync, not_expired);

        work.last_sync = Instant::now() - work.sync_interval;
        let expired = work.last_sync;
        work.record(b"data", &meta, &entry).unwrap();
        assert!(work.last_sync > expired);

        work.last_sync = future;
        work.record(b"data", &meta, &entry).unwrap();
        assert_eq!(work.last_sync, future);
        let session = work.session.clone();
        drop(work);
        let mut work = Checkpoint::open(root, "init", None).unwrap();
        assert_eq!(work.session, session);
        assert_eq!(work.sync_interval, SYNC_INTERVAL);
        assert_eq!(work.reuse(b"data", &meta).unwrap().hash, entry.hash);
        work.finish().unwrap();
        assert!(!root.join(".keeper/init.work").exists());
    }
}
