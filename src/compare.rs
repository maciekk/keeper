use crate::{catalog::Catalog, encoding};
use std::io::{self, Write};

/// Path-sensitive comparison. Object presence elsewhere is not an exact replica.
pub fn report(left: &Catalog, right: &Catalog, out: &mut impl Write) -> io::Result<bool> {
    let (mut only_left, mut only_right, mut different, mut matching) = (0, 0, 0, 0);
    for (path, entry) in left {
        let label = encoding::encode(path);
        match right.get(path) {
            None => {
                only_left += 1;
                writeln!(out, "ONLY_LEFT\t{label}")?;
            }
            Some(other) if entry != other => {
                different += 1;
                writeln!(
                    out,
                    "DIFFERENT\t{label}\tleft={}\tleft_size={}\tright={}\tright_size={}",
                    entry.hash, entry.size, other.hash, other.size
                )?;
            }
            _ => matching += 1,
        }
    }
    for path in right.keys().filter(|path| !left.contains_key(*path)) {
        only_right += 1;
        writeln!(out, "ONLY_RIGHT\t{}", encoding::encode(path))?;
    }
    writeln!(
        out,
        "Comparison: {matching} matching, {only_left} only left, {only_right} only right, {different} different. No files or catalogs changed."
    )?;
    Ok(only_left + only_right + different > 0)
}
