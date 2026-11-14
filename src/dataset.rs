//! Resolve the nearest dataset without widening an implicit selection.
use crate::{accept, catalog, encoding};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub struct Selection {
    pub root: PathBuf,
    base: PathBuf,
}

impl Selection {
    pub fn resolve(explicit: Option<&Path>) -> Result<Self> {
        if let Some(path) = explicit {
            let root = path.canonicalize().context("resolve --root")?;
            ensure!(root.is_dir(), "dataset root must be a directory");
            return Ok(Self {
                root,
                base: PathBuf::new(),
            });
        }
        let cwd = std::env::current_dir()?.canonicalize()?;
        for candidate in cwd.ancestors() {
            match fs::symlink_metadata(candidate.join(".keeper")) {
                Ok(_) => {
                    // Any nearest marker is a boundary, even when malformed. Do
                    // not skip invalid metadata and act on an enclosing dataset.
                    catalog::metadata_dir(candidate).with_context(|| {
                        format!(
                            "invalid nearest dataset at {}; refusing ancestor fallback",
                            candidate.display()
                        )
                    })?;
                    return Ok(Self {
                        root: candidate.to_owned(),
                        base: cwd.strip_prefix(candidate)?.to_owned(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .context("inspect nearest dataset marker; refusing ancestor fallback");
                }
            }
        }
        bail!("no enclosing Keeper dataset found; run keeper init or select a dataset with --root")
    }

    /// No selectors means this subtree, never the whole discovered parent tree.
    /// Empty selection for a metadata-only marker remains empty.
    pub fn paths(&self, paths: Vec<PathBuf>, marker: bool) -> Result<Vec<PathBuf>> {
        if paths.is_empty() {
            return Ok(if marker {
                Vec::new()
            } else {
                vec![if self.base.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    self.base.clone()
                }]
            });
        }
        paths.into_iter().map(|path| self.rebase(&path)).collect()
    }

    fn rebase(&self, path: &Path) -> Result<PathBuf> {
        ensure!(!path.as_os_str().is_empty(), "empty selector");
        let mut rebased = self.base.clone();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(name) => rebased.push(name),
                Component::ParentDir => {
                    ensure!(
                        !rebased.as_os_str().is_empty(),
                        "selector escapes the dataset root"
                    );
                    // Never normalize away a symlink or a non-directory. This
                    // also allows parent-relative references from real subdirs.
                    let bytes = rebased.as_os_str().as_encoded_bytes();
                    encoding::validate_path(bytes)?;
                    let (_, _, meta) = accept::inspect(&self.root, bytes)?
                        .context("parent traversal requires an existing real directory")?;
                    ensure!(
                        meta.is_dir(),
                        "parent traversal requires a real directory, not a symlink or file"
                    );
                    rebased.pop();
                }
                _ => bail!("selectors must be relative paths within the dataset"),
            }
        }
        if rebased.as_os_str().is_empty() {
            Ok(PathBuf::from("."))
        } else {
            Ok(rebased)
        }
    }
}
