//! Filesystem-specific restrictions, not a general filesystem certification.
use anyhow::{Context, Result};
use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path};

// Linux superblock magic values. ntfs3 reports lowercase "ntfs", unlike
// the legacy NTFS driver. ntfs-3g normally appears as FUSE, whose identity
// alone cannot certify timestamp semantics.
const NTFS3: libc::c_long = 0x7366_746e;
const NTFS_LEGACY: libc::c_long = 0x5346_544e;
const MSDOS: libc::c_long = 0x4d44;
const EXFAT: libc::c_long = 0x2011_bab0;
const FUSE: libc::c_long = 0x6573_5546;

fn permits_reuse(magic: libc::c_long) -> bool {
    !matches!(magic, NTFS3 | NTFS_LEGACY | MSDOS | EXFAT | FUSE)
}

pub fn checkpoint_reuse_allowed(path: &Path) -> Result<bool> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: a NUL-terminated path and writable storage for statfs are provided.
    let result = unsafe { libc::statfs(path.as_ptr(), info.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .context("inspect filesystem for checkpoint reuse");
    }
    // SAFETY: successful statfs initialized the structure.
    Ok(permits_reuse(unsafe { info.assume_init() }.f_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_coarse_and_unidentified_fuse_filesystems_never_authorize_reuse() {
        for magic in [NTFS3, NTFS_LEGACY, MSDOS, EXFAT, FUSE] {
            assert!(!permits_reuse(magic));
        }
        // These are not qualifications: the stopped-writer requirement applies
        // even on filesystems not restricted by this known-risk policy.
        assert!(permits_reuse(0xef53)); // ext4
        assert!(permits_reuse(0x9123_683e)); // btrfs
    }

    #[test]
    fn failed_filesystem_inspection_is_not_permission_to_reuse() {
        assert!(checkpoint_reuse_allowed(Path::new("/does/not/exist/keeper-fs-test")).is_err());
    }
}
