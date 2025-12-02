use crate::handler::SyscallResult;
use crate::path_redirect::PathRedirector;
use crate::ptrace_util::{read_string, write_struct};
use crate::seccomp::{SeccompData, SeccompNotifResp};
use crate::state::get_virtual_fd;
use nix::unistd::Pid;
use std::ffi::CString;
use tracing::*;

// struct stat for x86_64
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Stat64 {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    pub __unused: [i64; 3],
}

const S_IFCHR: u32 = 0o020000;
const S_IFMT: u32 = 0o170000;

/// Handle newfstatat syscall (used by stat, lstat, fstatat)
pub fn handle_newfstatat(pid: Pid, data: &SeccompData) -> SyscallResult {
    let dirfd = data.args[0] as i32;
    let path_ptr = data.args[1] as usize;
    let statbuf_ptr = data.args[2] as usize;
    let flags = data.args[3] as i32;

    // Try to read the path - if we can't, let kernel handle it
    let path = match read_string(pid, path_ptr) {
        Ok(p) => p,
        Err(e) => {
            trace!(
                "newfstatat: failed to read path from pid {}: {} - continuing",
                pid, e
            );
            return SyscallResult::Response(SeccompNotifResp::new_continue());
        }
    };

    // Only intercept paths we care about
    if !should_fake_stat(&path) {
        return SyscallResult::Response(SeccompNotifResp::new_continue());
    }

    debug!("newfstatat({}, {:?}, flags={:#x})", dirfd, path, flags);

    // Get the redirected path
    let actual_path = PathRedirector::redirect(&path).unwrap_or_else(|| path.clone());

    // Do the real stat on the redirected path
    let c_path = match CString::new(actual_path.clone()) {
        Ok(p) => p,
        Err(_) => return SyscallResult::Response(SeccompNotifResp::new_continue()),
    };

    let mut stat_buf: libc::stat = unsafe { std::mem::zeroed() };
    let use_dirfd = if actual_path.starts_with('/') {
        libc::AT_FDCWD
    } else {
        dirfd
    };

    let ret = unsafe { libc::fstatat(use_dirfd, c_path.as_ptr(), &mut stat_buf, flags) };

    if ret < 0 {
        let err = std::io::Error::last_os_error();
        let errno = err.raw_os_error().unwrap_or(libc::EIO);
        debug!("newfstatat({}) failed: {}", actual_path, err);
        return SyscallResult::Response(SeccompNotifResp::new_error(errno));
    }

    // Fake the device info
    let fake_stat = fake_device_stat(&path, &stat_buf);

    // Write the faked stat back to the process
    if let Err(e) = write_struct(pid, statbuf_ptr, &fake_stat) {
        error!("Failed to write stat buffer: {}", e);
        return SyscallResult::Response(SeccompNotifResp::new_error(libc::EFAULT));
    }

    info!("newfstatat: faked {} as char device", path);
    SyscallResult::Response(SeccompNotifResp::new_success(0))
}

/// Handle fstat syscall
pub fn handle_fstat(pid: Pid, data: &SeccompData) -> SyscallResult {
    let fd = data.args[0] as i32;
    let statbuf_ptr = data.args[1] as usize;

    // Check if this FD is one of our virtual devices
    let ctx = match get_virtual_fd(pid, fd) {
        Some(ctx) => ctx,
        None => {
            // Not our FD, let kernel handle it
            return SyscallResult::Response(SeccompNotifResp::new_continue());
        }
    };

    debug!("fstat({}) - virtual device {}", fd, ctx.event_node);

    // Create a fake stat structure for a character device
    let mut fake_stat = Stat64::default();

    // Determine device numbers based on node type
    let (major, minor) = if ctx.event_node.starts_with("event") {
        let event_num: u64 = ctx
            .event_node
            .trim_start_matches("event")
            .parse()
            .unwrap_or(0);
        (13u64, 64 + event_num)
    } else if ctx.event_node.starts_with("js") {
        let js_num: u64 = ctx.event_node.trim_start_matches("js").parse().unwrap_or(0);
        (81u64, js_num)
    } else {
        (13u64, 64u64)
    };

    fake_stat.st_mode = S_IFCHR | 0o660; // Character device with rw-rw----
    fake_stat.st_rdev = makedev(major, minor);
    fake_stat.st_dev = makedev(0, 5); // devtmpfs
    fake_stat.st_ino = 1000 + minor; // Fake inode
    fake_stat.st_nlink = 1;
    fake_stat.st_uid = unsafe { libc::getuid() };
    fake_stat.st_gid = unsafe { libc::getgid() };
    fake_stat.st_blksize = 4096;

    // Set timestamps to now
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    fake_stat.st_atime = now.as_secs() as i64;
    fake_stat.st_mtime = now.as_secs() as i64;
    fake_stat.st_ctime = now.as_secs() as i64;

    // Write to process memory
    if let Err(e) = write_struct(pid, statbuf_ptr, &fake_stat) {
        error!("Failed to write fstat buffer: {}", e);
        return SyscallResult::Response(SeccompNotifResp::new_error(libc::EFAULT));
    }

    info!(
        "fstat: faked fd {} as char device ({}:{})",
        fd, major, minor
    );
    SyscallResult::Response(SeccompNotifResp::new_success(0))
}

/// Check if we should fake stat for this path
fn should_fake_stat(path: &str) -> bool {
    // Only fake stat for actual device nodes, not directories or other paths
    if path.starts_with("/dev/input/event") {
        // Make sure it's eventN where N is a number
        let suffix = path.strip_prefix("/dev/input/event").unwrap_or("");
        return !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit());
    }
    if path.starts_with("/dev/input/js") {
        let suffix = path.strip_prefix("/dev/input/js").unwrap_or("");
        return !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit());
    }
    if path == "/dev/uinput" {
        return true;
    }
    false
}

/// Create a fake stat structure that looks like a character device
fn fake_device_stat(path: &str, real_stat: &libc::stat) -> Stat64 {
    let mut fake = Stat64::default();

    // Copy basic fields from real stat
    fake.st_dev = real_stat.st_dev;
    fake.st_ino = real_stat.st_ino;
    fake.st_nlink = real_stat.st_nlink as u64;
    fake.st_uid = real_stat.st_uid;
    fake.st_gid = real_stat.st_gid;
    fake.st_size = real_stat.st_size;
    fake.st_blksize = real_stat.st_blksize;
    fake.st_blocks = real_stat.st_blocks;
    fake.st_atime = real_stat.st_atime;
    fake.st_atime_nsec = real_stat.st_atime_nsec;
    fake.st_mtime = real_stat.st_mtime;
    fake.st_mtime_nsec = real_stat.st_mtime_nsec;
    fake.st_ctime = real_stat.st_ctime;
    fake.st_ctime_nsec = real_stat.st_ctime_nsec;

    // Determine device numbers
    let (major, minor) = if path.starts_with("/dev/input/event") {
        let event_num: u64 = path
            .trim_start_matches("/dev/input/event")
            .parse()
            .unwrap_or(0);
        (13u64, 64 + event_num)
    } else if path.starts_with("/dev/input/js") {
        let js_num: u64 = path
            .trim_start_matches("/dev/input/js")
            .parse()
            .unwrap_or(0);
        (81u64, js_num)
    } else if path == "/dev/uinput" {
        (10u64, 223u64)
    } else {
        (13u64, 64u64)
    };

    // Override mode to be a character device
    fake.st_mode = S_IFCHR | (real_stat.st_mode & 0o7777);
    fake.st_rdev = makedev(major, minor);

    fake
}

/// Create a device number from major and minor
fn makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xfffff000) << 32)
        | ((major & 0x00000fff) << 8)
        | ((minor & 0xffffff00) << 12)
        | (minor & 0x000000ff)
}
