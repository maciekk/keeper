use crate::{
    catalog::Catalog,
    display::{label, number},
    encoding,
};
use std::io::{self, Write};

/// Path-sensitive comparison. Object presence elsewhere is not an exact replica.
pub fn report(
    left: &Catalog,
    right: &Catalog,
    out: &mut impl Write,
    color: bool,
) -> io::Result<bool> {
    let (mut only_left, mut only_right, mut different, mut matching) = (0, 0, 0, 0);
    for (path, entry) in left {
        let label = encoding::encode(path);
        match right.get(path) {
            None => {
                only_left += 1;
                writeln!(
                    out,
                    "{}\t{label}",
                    crate::display::label("ONLY_LEFT", color)
                )?;
            }
            Some(other) if entry != other => {
                different += 1;
                writeln!(
                    out,
                    "{}\t{label}\tleft={}\tleft_size={}\tright={}\tright_size={}",
                    crate::display::label("DIFFERENT", color),
                    entry.hash,
                    crate::display::bytes(entry.size),
                    other.hash,
                    crate::display::bytes(other.size)
                )?;
            }
            _ => matching += 1,
        }
    }
    for path in right.keys().filter(|path| !left.contains_key(*path)) {
        only_right += 1;
        writeln!(
            out,
            "{}\t{}",
            label("ONLY_RIGHT", color),
            encoding::encode(path)
        )?;
    }
    writeln!(
        out,
        "{}: {} matching, {} only left, {} only right, {} different.\nNo files or catalogs changed.",
        label("Comparison", color),
        number(matching),
        number(only_left),
        number(only_right),
        number(different)
    )?;
    Ok(only_left + only_right + different > 0)
}
