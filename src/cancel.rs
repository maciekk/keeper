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

pub fn check() -> std::io::Result<()> {
    if CANCELLED.load(Ordering::Relaxed) {
        Err(std::io::Error::other(
            "cancelled; when using --resume, completed files are retained for the same command; current file must be reread",
        ))
    } else {
        Ok(())
    }
}
