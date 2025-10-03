use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

/// Lock file to prevent multiple manager instances
pub struct LockFile {
    _file: File,
}

impl LockFile {
    /// Acquire an exclusive lock on the given path
    pub fn acquire(path: &Path) -> anyhow::Result<Self> {
        let file = OpenOptions::new().create(true).write(true).open(path)?;

        #[cfg(unix)]
        {
            use libc::{LOCK_EX, LOCK_NB, flock};
            let fd = file.as_raw_fd();
            if unsafe { flock(fd, LOCK_EX | LOCK_NB) } != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Another manager instance is already running",
                )
                .into());
            }
        }

        tracing::info!("Acquired lock file: {}", path.display());

        Ok(Self { _file: file })
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        tracing::info!("Released lock file");
    }
}
