//! Optional plain-text progress on stderr; never mixed into comparison reports.
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

pub struct Progress<'a> {
    out: &'a mut dyn Write,
    enabled: bool,
    phase: &'static str,
    scope: &'static str,
    started: Instant,
    last: Instant,
    files: u64,
    bytes: u64,
    totals: Option<(u64, u64)>,
}

impl<'a> Progress<'a> {
    pub fn new(out: &'a mut dyn Write, enabled: bool) -> Self {
        Self {
            out,
            enabled,
            phase: "",
            scope: "",
            started: Instant::now(),
            last: Instant::now(),
            files: 0,
            bytes: 0,
            totals: None,
        }
    }

    pub fn set_scope(&mut self, scope: &'static str) {
        self.scope = scope;
    }

    pub fn begin(&mut self, phase: &'static str, totals: Option<(u64, u64)>) -> io::Result<()> {
        self.phase = phase;
        self.started = Instant::now();
        self.last = self.started;
        self.files = 0;
        self.bytes = 0;
        self.totals = totals;
        self.report("started")
    }

    pub fn advance(&mut self, files: u64, bytes: u64) -> io::Result<()> {
        self.files += files;
        self.bytes += bytes;
        if self.last.elapsed() >= Duration::from_secs(1) {
            self.report("running")?;
            self.last = Instant::now();
        }
        Ok(())
    }

    pub fn finish(&mut self) -> io::Result<()> {
        self.report("completed")
    }

    fn report(&mut self, state: &str) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        match self.totals {
            Some((files, bytes)) => writeln!(
                self.out,
                "{}{}: {state}; files={}/{}; read={}/{} bytes; {:.1} MiB/s; elapsed={:.1}s",
                self.scope,
                self.phase,
                self.files,
                files,
                self.bytes,
                bytes,
                self.bytes as f64 / elapsed.max(0.001) / 1048576.0,
                elapsed
            )?,
            None => writeln!(
                self.out,
                "{}{}: {state}; files={}; inventoried={} bytes (contents not read); elapsed={:.1}s",
                self.scope, self.phase, self.files, self.bytes, elapsed
            )?,
        }
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_final_counts_and_can_be_disabled() {
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, true);
        progress.begin("Hashing", Some((1, 42))).unwrap();
        progress.advance(0, 42).unwrap();
        progress.advance(1, 0).unwrap();
        progress.finish().unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Hashing: completed; files=1/1; read=42/42 bytes"));
        let mut output = Vec::new();
        let mut progress = Progress::new(&mut output, false);
        progress.begin("Preflight", None).unwrap();
        progress.advance(1, 42).unwrap();
        progress.finish().unwrap();
        assert!(output.is_empty());
    }
}
