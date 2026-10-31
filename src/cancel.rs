//! Signal handlers only set a flag; normal control flow handles cleanup.
use std::sync::atomic::{AtomicBool, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);

extern "C" fn cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::Relaxed);
}

pub fn install() -> std::io::Result<()> {
    // SAFETY: initialized sigaction, valid handler, and no borrowed state in handler.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = cancel as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        for signal in [libc::SIGINT, libc::SIGTERM] {
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("cancelled; when using --resume, completed files are retained for the same command; current file must be reread")
    }
}

impl std::error::Error for Cancelled {}

pub fn is_cancellation(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<Cancelled>()
            || cause
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::get_ref)
                .is_some_and(|inner| inner.is::<Cancelled>())
    })
}

pub fn check() -> std::io::Result<()> {
    if CANCELLED.load(Ordering::Relaxed) {
        Err(std::io::Error::other(Cancelled))
    } else {
        Ok(())
    }
}
