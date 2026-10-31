//! In-place terminal progress on stderr; plain phase boundaries when redirected.
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

// Avoid adding a second indicator for the usual small-file workload.
const LARGE_FILE_BYTES: u64 = 256 * 1024 * 1024;

struct FileProgress {
    size: u64,
    read: u64,
    started: Instant,
}

pub struct Progress<'a> {
    out: &'a mut dyn Write,
    enabled: bool,
    terminal: bool,
    color: bool,
    active: bool,
    phase: &'static str,
    scope: &'static str,
    started: Instant,
    last: Instant,
    files: u64,
    bytes: u64,
    totals: Option<(u64, u64)>,
    current_file: Option<FileProgress>,
}

impl<'a> Progress<'a> {
    pub fn new(out: &'a mut dyn Write, enabled: bool) -> Self {
        Self {
            out,
            enabled,
            terminal: false,
            color: false,
            active: false,
            phase: "",
            scope: "",
            started: Instant::now(),
            last: Instant::now(),
            files: 0,
            bytes: 0,
            totals: None,
            current_file: None,
        }
    }

    pub fn terminal(mut self, terminal: bool) -> Self {
        self.terminal = terminal;
        self
    }

    pub fn color(mut self, color: bool) -> Self {
        self.color = color;
        self
    }

    pub fn set_scope(&mut self, scope: &'static str) {
        self.scope = scope;
    }

    pub fn begin(&mut self, phase: &'static str, totals: Option<(u64, u64)>) -> io::Result<()> {
        crate::cancel::check()?;
        self.phase = phase;
        self.started = Instant::now();
        self.last = self.started;
        self.files = 0;
        self.bytes = 0;
        self.totals = totals;
        self.current_file = None;
        self.report("started")
    }

    pub fn begin_file(&mut self, size: u64) {
        self.current_file =
            (self.enabled && self.terminal && size >= LARGE_FILE_BYTES).then(|| FileProgress {
                size,
                read: 0,
                started: Instant::now(),
            });
    }

    pub fn end_file(&mut self) {
        self.current_file = None;
    }

    pub fn advance(&mut self, files: u64, bytes: u64) -> io::Result<()> {
        crate::cancel::check()?;
        self.files += files;
        self.bytes += bytes;
        if let Some(file) = &mut self.current_file {
            file.read += bytes;
        }
        if self.last.elapsed() >= Duration::from_secs(1) {
            self.report("running")?;
            self.last = Instant::now();
        }
        Ok(())
    }

    pub fn finish(&mut self) -> io::Result<()> {
        self.end_file();
        self.report("completed")
    }

    fn report(&mut self, state: &str) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        // Redirected runs retain phase boundaries, not a stream of repetitive updates.
        if !self.terminal && state == "running" {
            return Ok(());
        }
        if self.terminal {
            write!(self.out, "\r\x1b[2K")?;
            self.active = true;
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        let mut line = Vec::new();
        match self.totals {
            Some((files, bytes)) => write!(
                line,
                "{}{}: {state}; files={}/{}; read={}/{}; {:.1} MiB/s; elapsed={}",
                self.scope,
                self.phase,
                crate::display::number(self.files),
                crate::display::number(files),
                crate::display::bytes(self.bytes),
                crate::display::bytes(bytes),
                self.bytes as f64 / elapsed.max(0.001) / 1048576.0,
                crate::display::duration(elapsed)
            )?,
            None => write!(
                line,
                "{}{}: {state}; files={}; inventoried={}; elapsed={}",
                self.scope,
                self.phase,
                crate::display::number(self.files),
                crate::display::bytes(self.bytes),
                crate::display::duration(elapsed)
            )?,
        }
        if let Some((files, bytes)) = self.totals {
            let fraction = if bytes > 0 {
                self.bytes as f64 / bytes as f64
            } else if files > 0 {
                self.files as f64 / files as f64
            } else {
                1.0
            }
            .clamp(0.0, 1.0);
            write!(line, "; {:.1}%", fraction * 100.0)?;
            if fraction > 0.0 && fraction < 1.0 {
                write!(
                    line,
                    "; ETA={}",
                    crate::display::duration(elapsed * (1.0 - fraction) / fraction)
                )?;
            }
        }
        let mut suffix = Vec::new();
        if state == "running"
            && let Some(file) = &self.current_file
            && file.started.elapsed() >= Duration::from_secs(1)
        {
            let fraction = (file.read as f64 / file.size as f64).clamp(0.0, 1.0);
            let filled = (fraction * 10.0).floor() as usize;
            write!(
                suffix,
                "; [{}{}]",
                "*".repeat(filled),
                ".".repeat(10 - filled)
            )?;
        }
        // Fit plain ASCII before styling; ETA is protected from truncation.
        fit_progress(
            &mut line,
            &suffix,
            if self.terminal {
                terminal_columns()
            } else {
                None
            },
        );
        let text = String::from_utf8(line).expect("progress is ASCII");
        self.out.write_all(
            crate::display::progress_line(&text, self.color, state == "completed").as_bytes(),
        )?;
        if !self.terminal || state == "completed" {
            writeln!(self.out)?;
            self.active = false;
        }
        self.out.flush()
    }
}

fn terminal_columns() -> Option<usize> {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    // SAFETY: ioctl receives a valid writable winsize pointer; read only on success.
    if unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0 {
        let columns = unsafe { size.assume_init() }.ws_col as usize;
        (columns > 0).then_some(columns)
    } else {
        None
    }
}

fn fit_progress(line: &mut Vec<u8>, suffix: &[u8], columns: Option<usize>) {
    let Some(columns) = columns else {
        line.extend_from_slice(suffix);
        return;
    };
    let limit = columns.saturating_sub(1);
    let eta = line
        .windows(6)
        .rposition(|part| part == b"; ETA=")
        .map(|index| line.split_off(index))
        .unwrap_or_default();
    // Retain a little context before the ETA/bar. Drop the bar first if needed.
    let suffix = if eta.len() + suffix.len() + 24 <= limit {
        suffix
    } else {
        &[]
    };
    if !eta.is_empty() && eta.len() >= limit {
        // A full ETA cannot physically fit in a smaller terminal. Prefer the
        // complete value to an ellipsis, even if this pathological case wraps.
        line.clear();
        line.extend_from_slice(&eta[2..]);
        return;
    }
    fit_line(line, limit.saturating_sub(eta.len() + suffix.len()) + 1);
    line.extend_from_slice(&eta);
    line.extend_from_slice(suffix);
}

fn fit_line(line: &mut Vec<u8>, columns: usize) {
    let limit = columns.saturating_sub(1);
    if line.len() > limit {
        line.truncate(limit);
        if limit >= 3 {
            line[limit - 3..].copy_from_slice(b"...");
        }
    }
}

impl Drop for Progress<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = writeln!(self.out);
            let _ = self.out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_terminal_does_not_wrap() {
        let mut line = b"Hashing: a long progress update".to_vec();
        fit_line(&mut line, 12);
        assert_eq!(line, b"Hashing:...");
        fit_line(&mut line, 1);
        assert!(line.is_empty());
    }

    #[test]
    fn large_file_bar_tracks_bytes_and_clears_between_files() {
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, true).terminal(true);
        let size = LARGE_FILE_BYTES * 4;
        progress.begin("Hashing", Some((2, size * 2))).unwrap();
        progress.begin_file(size);
        progress.current_file.as_mut().unwrap().started -= Duration::from_secs(2);
        progress.advance(0, size / 2).unwrap();
        progress.report("running").unwrap();
        assert_eq!(progress.files, 0);
        assert_eq!(progress.current_file.as_ref().unwrap().read, size / 2);
        progress.end_file();
        progress.advance(1, 0).unwrap();
        progress.begin_file(42);
        assert!(progress.current_file.is_none());
        progress.report("running").unwrap();
        drop(progress);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("; [*****.....]"));
        assert!(output.find("; ETA=").unwrap() < output.find("; [").unwrap());
        assert_eq!(output.matches("; [").count(), 1);
        assert!(!output.contains("512.00 MiB/1.00 GiB"));
    }

    #[test]
    fn file_bar_waits_for_a_long_read_and_never_appears_in_redirected_logs() {
        for terminal in [false, true] {
            let mut output = Vec::new();
            let mut progress = Progress::new(&mut output, true).terminal(terminal);
            progress
                .begin("Hashing", Some((1, LARGE_FILE_BYTES)))
                .unwrap();
            progress.begin_file(LARGE_FILE_BYTES);
            progress.report("running").unwrap();
            progress.finish().unwrap();
            assert!(progress.current_file.is_none());
            drop(progress);
            assert!(!String::from_utf8(output).unwrap().contains("; ["));
        }
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, false).terminal(true);
        progress.begin_file(LARGE_FILE_BYTES);
        assert!(progress.current_file.is_none());
    }

    #[test]
    fn file_bar_remains_visible_without_wrapping_on_smaller_terminals() {
        let base = b"Hashing: running; files=123/1000; read=1.00 GiB/20.00 GiB; elapsed=1h; ETA=2h";
        let suffix = b"; [*****.....]";
        let mut line = base.to_vec();
        fit_progress(&mut line, suffix, Some(100));
        assert!(line.len() < 100);
        assert!(line.ends_with(suffix));
        let mut line = base.to_vec();
        fit_progress(&mut line, suffix, Some(20));
        assert!(line.len() < 20);
        assert!(!line.windows(3).any(|part| part == b"; ["));
        assert!(line.ends_with(b"; ETA=2h"));
        let mut line = base.to_vec();
        fit_progress(&mut line, suffix, None);
        assert!(line.starts_with(base));
        assert!(line.ends_with(suffix));
    }

    #[test]
    fn eta_is_never_shortened_with_or_without_a_file_bar() {
        let base = b"Hashing: running; files=123/1000; read=1.00 GiB/20.00 GiB; elapsed=1h; ETA=2h 38m 10s";
        let bar = b"; [*****.....]";
        for suffix in [bar.as_slice(), b"".as_slice()] {
            for columns in 1..200 {
                let mut line = base.to_vec();
                fit_progress(&mut line, suffix, Some(columns));
                let line = String::from_utf8(line).unwrap();
                assert!(line.contains("ETA=2h 38m 10s"), "width={columns}: {line}");
                if columns > "ETA=2h 38m 10s".len() + 2 {
                    assert!(line.len() < columns, "width={columns}: {line}");
                }
            }
        }
    }

    #[test]
    fn terminal_updates_replace_one_line_and_finish_with_newline() {
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, true).terminal(true);
        progress.begin("Hashing", Some((2, 1000))).unwrap();
        progress.advance(1, 500).unwrap();
        progress.report("running").unwrap();
        progress.advance(1, 500).unwrap();
        progress.finish().unwrap();
        drop(progress);
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.matches('\n').count(), 1);
        assert!(text.starts_with("\r\x1b[2K"));
        assert_eq!(text.matches("\r\x1b[2K").count(), 3);
        assert!(text.contains("50.0%; ETA="));
        assert!(text.ends_with("100.0%\n"));
    }

    #[test]
    fn long_running_progress_formats_eta_and_elapsed_as_durations() {
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, true).terminal(true);
        progress.begin("Hashing", Some((2, 200))).unwrap();
        progress.advance(1, 100).unwrap();
        progress.started = Instant::now() - Duration::from_secs(9_490);
        progress.report("running").unwrap();
        drop(progress);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("elapsed=3h"));
        assert!(text.contains("ETA=3h"));
        assert!(!text.contains("9490s"));
    }

    #[test]
    fn left_and_right_terminal_phases_are_contiguous() {
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, true).terminal(true);
        for scope in ["Left ", "Right "] {
            progress.set_scope(scope);
            progress.begin("Preflight", None).unwrap();
            progress.finish().unwrap();
        }
        drop(progress);
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains("\n\n"));
        assert_eq!(text.matches('\n').count(), 2);
        assert!(text.starts_with("\r\x1b[2KLeft Preflight"));
        assert!(text.contains("\n\r\x1b[2KRight Preflight"));
    }

    #[test]
    fn redirected_progress_omits_repetitive_updates_and_terminal_controls() {
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, true);
        progress.begin("Hashing", Some((1, 42))).unwrap();
        progress.report("running").unwrap();
        progress.advance(1, 42).unwrap();
        progress.finish().unwrap();
        drop(progress);
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(!text.contains(['\r', '\x1b']));
        assert!(!text.contains("running"));
    }

    #[test]
    fn interrupted_terminal_progress_leaves_a_clean_line() {
        let mut output = Vec::new();
        {
            let mut progress = Progress::new(&mut output, true).terminal(true);
            progress.begin("Hashing", None).unwrap();
        }
        assert!(output.ends_with(b"\n"));
    }

    #[test]
    fn reports_final_counts_and_can_be_disabled() {
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, true);
        progress.begin("Hashing", Some((1, 42))).unwrap();
        progress.advance(0, 42).unwrap();
        progress.advance(1, 0).unwrap();
        progress.finish().unwrap();
        drop(progress);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Hashing: completed; files=1/1; read=42 B/42 B"));
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, false);
        progress.begin("Preflight", None).unwrap();
        progress.advance(1, 42).unwrap();
        progress.finish().unwrap();
        drop(progress);
        assert!(output.is_empty());
    }
}
