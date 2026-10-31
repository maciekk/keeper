//! Explicit root-level scan scope. No implicit housekeeping exclusions.
use crate::{catalog, encoding};
use anyhow::{Context, Result, ensure};
use std::{fs, path::Path};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub source: Option<Vec<u8>>,
    rules: Vec<Rule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    pattern: Vec<u8>,
    directory: bool,
    anchored: bool,
}

impl Policy {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(".keeperignore");
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).context("inspect .keeperignore"),
            Ok(meta) => ensure!(
                meta.is_file() && meta.len() <= 1024 * 1024,
                ".keeperignore must be a regular file of at most 1 MiB (not a symlink)"
            ),
        }
        Self::parse(catalog::read_regular(&path).context("read .keeperignore")?)
    }

    pub fn parse(source: Vec<u8>) -> Result<Self> {
        ensure!(source.len() <= 1024 * 1024, ".keeperignore exceeds 1 MiB");
        let text = std::str::from_utf8(&source).context(".keeperignore must be UTF-8")?;
        let mut rules = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim_end_matches([' ', '\t', '\r']);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            ensure!(
                !line.starts_with('!') && !line.contains(['\\', '[', ']', '\0']),
                ".keeperignore line {}: negation, escapes, character classes and NUL are not supported",
                index + 1
            );
            let directory = line.ends_with('/');
            let anchored = line.starts_with('/');
            let pattern = line.strip_prefix('/').unwrap_or(line);
            let pattern = pattern.strip_suffix('/').unwrap_or(pattern);
            ensure!(
                !pattern.is_empty()
                    && pattern
                        .split('/')
                        .all(|part| !part.is_empty() && part != "." && part != ".."),
                ".keeperignore line {}: invalid path pattern",
                index + 1
            );
            ensure!(
                pattern
                    .split('/')
                    .all(|part| !part.contains("**") || part == "**"),
                ".keeperignore line {}: ** must occupy an entire path component",
                index + 1
            );
            rules.push(Rule {
                pattern: pattern.as_bytes().to_vec(),
                directory,
                anchored: anchored || pattern.contains('/'),
            });
        }
        Ok(Self {
            source: Some(source),
            rules,
        })
    }

    pub fn id(&self) -> String {
        match &self.source {
            None => "none".into(),
            Some(source) => format!("b3:{}", blake3::hash(source).to_hex()),
        }
    }

    pub fn summary(&self) -> String {
        let patterns = self
            .rules
            .iter()
            .map(|rule| {
                format!(
                    "{}{}{}",
                    if rule.anchored { "/" } else { "" },
                    encoding::encode(&rule.pattern),
                    if rule.directory { "/" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "patterns: {}",
            if patterns.is_empty() {
                "(none)"
            } else {
                &patterns
            }
        )
    }

    pub fn notes(&self, excluded: Option<usize>) -> Vec<String> {
        if self.source.is_none() {
            return Vec::new();
        }
        let mut notes = vec![format!(
            ".keeperignore {}; configuration is not dataset content.",
            self.summary()
        )];
        if let Some(count) = excluded {
            notes.push(format!(
                "{} excluded entry/subtree root(s); contents and byte sizes not counted.",
                crate::display::number(count)
            ));
        }
        notes
    }

    pub fn ensure_current(&self, root: &Path) -> Result<()> {
        ensure!(
            *self == Self::load(root)?,
            ".keeperignore changed during the operation; stop writers and retry"
        );
        Ok(())
    }

    pub fn excludes(&self, path: &[u8], is_dir: bool) -> bool {
        // Check ancestors too: an excluded directory excludes its whole subtree.
        for end in path
            .iter()
            .enumerate()
            .filter_map(|(i, &b)| (b == b'/').then_some(i))
            .chain(std::iter::once(path.len()))
        {
            let prefix = &path[..end];
            let directory = end < path.len() || is_dir;
            for rule in &self.rules {
                if rule.directory && !directory {
                    continue;
                }
                let candidate = if rule.anchored {
                    prefix
                } else {
                    prefix.rsplit(|&b| b == b'/').next().unwrap()
                };
                if glob(&rule.pattern, candidate) {
                    return true;
                }
            }
        }
        false
    }
}

// Match components separately so **/ can match zero directories, but cannot
// accidentally swallow part of the next filename without a separator.
fn glob(pattern: &[u8], path: &[u8]) -> bool {
    if !pattern.contains(&b'*') && !pattern.contains(&b'?') {
        return pattern == path;
    }
    let parts: Vec<_> = path.split(|&b| b == b'/').collect();
    let mut next = vec![false; parts.len() + 1];
    next[parts.len()] = true;
    for (index, component) in pattern.rsplit(|&b| b == b'/').enumerate() {
        let mut row = vec![false; parts.len() + 1];
        for j in (0..=parts.len()).rev() {
            row[j] = if component == b"**" {
                (index > 0 && next[j]) || (j < parts.len() && (next[j + 1] || row[j + 1]))
            } else {
                j < parts.len() && next[j + 1] && segment(component, parts[j])
            };
        }
        next = row;
    }
    next[0]
}

fn segment(pattern: &[u8], name: &[u8]) -> bool {
    let mut next = vec![false; name.len() + 1];
    next[name.len()] = true;
    for &byte in pattern.iter().rev() {
        let mut row = vec![false; name.len() + 1];
        for j in (0..=name.len()).rev() {
            row[j] = match byte {
                b'*' => next[j] || (j < name.len() && row[j + 1]),
                b'?' => j < name.len() && next[j + 1],
                byte => j < name.len() && byte == name[j] && next[j + 1],
            };
        }
        next = row;
    }
    next[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rooted_basename_directory_and_recursive_patterns() {
        let policy = Policy::parse(
            b"# scope\n/lost+found/\n*.tmp\ncache/\nphotos/**/scratch?.jpg\n".to_vec(),
        )
        .unwrap();
        for (path, dir) in [
            (b"lost+found".as_slice(), true),
            (b"lost+found/a", false),
            (b"nested/a.tmp", false),
            (b"x/cache/file", false),
            (b"photos/scratch1.jpg", false),
            (b"photos/a/b/scratch2.jpg", false),
        ] {
            assert!(policy.excludes(path, dir), "{path:?}");
        }
        for path in [
            b"nested/lost+found/a".as_slice(),
            b"lost+found",
            b"photos/a/keep.jpg",
            b"nested/photos/scratch1.jpg",
        ] {
            assert!(!policy.excludes(path, false), "{path:?}");
        }
    }
    #[test]
    fn recursive_globs_respect_component_boundaries() {
        for path in [b"a/b".as_slice(), b"a/x/b", b"a/x/y/b"] {
            assert!(glob(b"a/**/b", path));
        }
        assert!(!glob(b"a/**/b", b"a/xb"));
        assert!(!glob(b"a/**", b"a"));
        assert!(glob(b"a/**", b"a/file"));
        assert!(glob(b"a/**", b"a/x/file"));
        assert!(!glob(b"a/*", b"a/x/file"));
        assert!(glob(b"a/?", b"a/x"));
        assert!(!glob(b"a/?", b"a/xy"));
    }

    #[test]
    fn malformed_or_unsupported_patterns_fail_loudly() {
        for pattern in ["!keep", "[ab]", "a\\b", "/", "a//b", "../a", "a**b", "a\0b"] {
            assert!(
                Policy::parse(pattern.as_bytes().to_vec()).is_err(),
                "{pattern:?}"
            );
        }
        assert_ne!(Policy::default(), Policy::parse(Vec::new()).unwrap());
    }
}
