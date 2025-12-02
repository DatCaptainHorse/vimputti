use crate::ioctl_handler::{IoctlResult, handle_ioctl};
use crate::path_redirect::PathRedirector;
use crate::ptrace_util::read_string;
use crate::seccomp::{SeccompData, SeccompNotifResp};
use crate::socket_handler::{handle_bind, handle_socket};
use crate::stat_handler::{handle_fstat, handle_newfstatat};
use crate::state::{DeviceType, VirtualFdContext, register_virtual_fd};
use anyhow::{Result, anyhow};
use nix::unistd::Pid;
use std::ffi::CString;
use std::io::Read;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use tracing::*;

pub enum SyscallResult {
    Response(SeccompNotifResp),
    AlreadyHandled,
}

pub fn handle_syscall(pid: Pid, data: &SeccompData) -> SyscallResult {
    let nr = data.nr as i64;

    match nr {
        libc::SYS_openat => handle_openat(pid, data),
        libc::SYS_ioctl => handle_ioctl_syscall(pid, data),
        libc::SYS_newfstatat => handle_newfstatat(pid, data),
        libc::SYS_socket => handle_socket(pid, data),
        libc::SYS_bind => handle_bind(pid, data),
        _ => {
            // Unknown syscall that somehow got through our filter
            // Let the kernel handle it
            debug!("Unfiltered syscall {} - continuing", nr);
            SyscallResult::Response(SeccompNotifResp::new_continue())
        }
    }
}

fn handle_ioctl_syscall(pid: Pid, data: &SeccompData) -> SyscallResult {
    let fd = data.args[0] as i32;
    let cmd = data.args[1] as u32;
    let arg = data.args[2];

    match handle_ioctl(pid, fd, cmd, arg) {
        IoctlResult::Handled(resp) => SyscallResult::Response(resp),
        IoctlResult::NotVirtualFd => {
            // Not a virtual FD - let the kernel handle it
            SyscallResult::Response(SeccompNotifResp::new_continue())
        }
    }
}

fn handle_openat(pid: Pid, data: &SeccompData) -> SyscallResult {
    let dirfd = data.args[0] as i32;
    let path_ptr = data.args[1] as usize;
    let flags = data.args[2] as i32;
    let mode = data.args[3] as u32;

    // Try to read the path - if we can't (e.g., permission denied in container),
    // let the kernel handle it
    let path = match read_string(pid, path_ptr) {
        Ok(p) => p,
        Err(e) => {
            trace!(
                "openat: failed to read path from pid {}: {} - continuing",
                pid, e
            );
            return SyscallResult::Response(SeccompNotifResp::new_continue());
        }
    };

    trace!("openat({}, {:?}, {:#x}, {:#o})", dirfd, path, flags, mode);

    // Check if this is a virtual input device that needs socket connection
    if PathRedirector::is_input_device(&path) {
        return handle_virtual_device_open(pid, &path, flags);
    }

    // Check if path needs redirection
    if let Some(redirected) = PathRedirector::redirect(&path) {
        trace!("Redirecting open: {} -> {}", path, redirected);
        return open_and_inject_file(pid, &redirected, dirfd, flags, mode);
    }

    // Path doesn't need any special handling - let kernel do it
    SyscallResult::Response(SeccompNotifResp::new_continue())
}

/// Handle opening a virtual input device (connects to manager socket)
fn handle_virtual_device_open(pid: Pid, original_path: &str, _flags: i32) -> SyscallResult {
    let redirected_path = match PathRedirector::redirect(original_path) {
        Some(p) => p,
        None => {
            error!(
                "is_input_device returned true but no redirect for {}",
                original_path
            );
            return SyscallResult::Response(SeccompNotifResp::new_error(libc::ENOENT));
        }
    };

    trace!(
        "Opening virtual device: {} -> {}",
        original_path, redirected_path
    );

    // Determine device type from path
    let device_type = if original_path == "/dev/uinput" {
        DeviceType::Uinput
    } else if original_path.starts_with("/dev/input/js") {
        DeviceType::Joystick
    } else {
        DeviceType::Event
    };

    // Connect to the Unix socket
    let mut stream = match UnixStream::connect(&redirected_path) {
        Ok(s) => s,
        Err(e) => {
            trace!("Failed to connect to {}: {}", redirected_path, e);
            return SyscallResult::Response(SeccompNotifResp::new_error(
                e.raw_os_error().unwrap_or(libc::ENOENT),
            ));
        }
    };

    debug!("Connected to socket at {}", redirected_path);

    // Perform handshake - read the DeviceHandshake from manager
    let handshake = match receive_handshake(&mut stream) {
        Ok(h) => h,
        Err(e) => {
            error!("Handshake failed for {}: {}", redirected_path, e);
            return SyscallResult::Response(SeccompNotifResp::new_error(libc::EIO));
        }
    };

    info!(
        "Handshake complete: device_id={}, name={}",
        handshake.device_id, handshake.config.name
    );

    // Extract the event node name from the path
    let event_node = redirected_path
        .rsplit('/')
        .next()
        .unwrap_or("unknown")
        .to_string();

    // Get the raw FD from the stream
    let socket_fd = stream.as_raw_fd();

    // Duplicate the FD so we can keep our copy for tracking
    let our_fd = unsafe { libc::dup(socket_fd) };
    if our_fd < 0 {
        error!(
            "Failed to dup socket fd: {}",
            std::io::Error::last_os_error()
        );
        return SyscallResult::Response(SeccompNotifResp::new_error(libc::EIO));
    }

    // Inject the socket FD into the target process
    let target_fd = match inject_fd(socket_fd) {
        Ok(fd) => fd,
        Err(e) => {
            error!("Failed to inject fd: {}", e);
            unsafe { libc::close(our_fd) };
            return SyscallResult::Response(SeccompNotifResp::new_error(libc::EIO));
        }
    };

    // Prevent stream from closing the FD we just injected
    std::mem::forget(stream);

    // Register this FD for ioctl interception
    let ctx = VirtualFdContext {
        event_node,
        device_type,
        device_id: handshake.device_id,
        manager_fd: our_fd,
        config: handshake.config,
    };
    register_virtual_fd(pid, target_fd, ctx);

    info!(
        "Virtual device opened: pid={}, target_fd={}, device_id={}",
        pid, target_fd, handshake.device_id
    );

    SyscallResult::Response(SeccompNotifResp::new_success(target_fd as i64))
}

/// Receive and parse DeviceHandshake from the manager
fn receive_handshake(stream: &mut UnixStream) -> Result<vimputti::protocol::DeviceHandshake> {
    // Set a reasonable timeout for handshake
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;

    // Read length prefix (4 bytes, little-endian)
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;

    debug!("Handshake: expecting {} bytes", len);

    if len > 1024 * 1024 {
        return Err(anyhow!("Handshake message too large: {} bytes", len));
    }

    // Read the JSON payload
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;

    // Parse as DeviceHandshake
    let handshake: vimputti::protocol::DeviceHandshake = serde_json::from_slice(&payload)?;

    // Clear the timeout for normal operation
    stream.set_read_timeout(None)?;

    Ok(handshake)
}

/// Open a regular file and inject the FD
fn open_and_inject_file(
    pid: Pid,
    actual_path: &str,
    dirfd: i32,
    flags: i32,
    mode: u32,
) -> SyscallResult {
    let c_path = match CString::new(actual_path.to_string()) {
        Ok(p) => p,
        Err(_) => return SyscallResult::Response(SeccompNotifResp::new_error(libc::EINVAL)),
    };

    let use_dirfd = if actual_path.starts_with('/') {
        libc::AT_FDCWD
    } else {
        dirfd
    };

    let fd = unsafe { libc::openat(use_dirfd, c_path.as_ptr(), flags, mode) };

    if fd < 0 {
        let err = std::io::Error::last_os_error();
        let errno = err.raw_os_error().unwrap_or(libc::EIO);
        debug!("openat({}) failed: {} (errno={})", actual_path, err, errno);
        return SyscallResult::Response(SeccompNotifResp::new_error(errno));
    }

    debug!("Opened {} as fd {} in supervisor", actual_path, fd);

    match inject_fd(fd) {
        Ok(target_fd) => {
            unsafe { libc::close(fd) };
            debug!("Injected fd {} -> {} in target pid {}", fd, target_fd, pid);
            SyscallResult::Response(SeccompNotifResp::new_success(target_fd as i64))
        }
        Err(e) => {
            error!("Failed to inject fd: {}", e);
            unsafe { libc::close(fd) };
            SyscallResult::Response(SeccompNotifResp::new_error(libc::EIO))
        }
    }
}

pub fn inject_fd(our_fd: RawFd) -> anyhow::Result<RawFd> {
    let (notif_fd, notif_id) = crate::get_notif_context();

    if notif_fd < 0 {
        return Err(anyhow!("No notification context available"));
    }

    crate::seccomp::notif_addfd(notif_fd, notif_id, our_fd)
}
