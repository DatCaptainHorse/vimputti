use anyhow::{Result, anyhow};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, execvp, fork};
use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::os::unix::io::RawFd;
use tracing::*;
use tracing_subscriber::EnvFilter;

mod handler;
mod ioctl_handler;
mod path_redirect;
mod ptrace_util;
mod seccomp;
mod socket_handler;
mod stat_handler;
mod state;
mod udev_forwarder;

use handler::SyscallResult;
use seccomp::*;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<CString> = std::env::args()
        .skip(1)
        .map(|s| CString::new(s).unwrap())
        .collect();

    if args.is_empty() {
        anyhow::bail!("Usage: vimputti-launcher <program> [args...]");
    }

    let program = args[0].clone();
    info!("Launching {:?} with seccomp filter", program);

    let (parent_sock, child_sock) = nix::sys::socket::socketpair(
        nix::sys::socket::AddressFamily::Unix,
        nix::sys::socket::SockType::Stream,
        None,
        nix::sys::socket::SockFlag::empty(),
    )?;

    let parent_sock_fd = parent_sock.as_raw_fd();
    let child_sock_fd = child_sock.as_raw_fd();

    let parent_sock_fd = unsafe { libc::dup(parent_sock_fd) };
    let child_sock_fd = unsafe { libc::dup(child_sock_fd) };

    drop(parent_sock);
    drop(child_sock);

    debug!(
        "Created socketpair: parent_fd={}, child_fd={}",
        parent_sock_fd, child_sock_fd
    );

    match unsafe { fork()? } {
        ForkResult::Parent { child } => {
            debug!("Parent: forked child {}", child);

            unsafe { libc::close(child_sock_fd) };

            debug!("Parent: waiting for notification fd from child...");

            let notif_fd = recv_fd(parent_sock_fd)?;

            debug!("Parent: received fd: {}", notif_fd);

            unsafe { libc::close(parent_sock_fd) };

            if notif_fd < 0 {
                return Err(anyhow!("Child failed to install seccomp filter"));
            }

            info!("Received notification fd: {}", notif_fd);

            // Start udev event forwarder
            udev_forwarder::start_udev_forwarder();

            handle_notifications(child, notif_fd)?;
        }
        ForkResult::Child => {
            debug!("Child: starting");

            unsafe { libc::close(parent_sock_fd) };

            debug!("Child: installing seccomp filter...");

            let notif_fd = match install_filter() {
                Ok(fd) => fd,
                Err(e) => {
                    error!("Failed to install filter: {}", e);
                    let _ = send_fd(child_sock_fd, -1);
                    unsafe { libc::close(child_sock_fd) };
                    std::process::exit(1);
                }
            };

            debug!("Child: seccomp filter installed, notif_fd={}", notif_fd);

            debug!("Child: sending notif_fd to parent...");
            if let Err(e) = send_fd(child_sock_fd, notif_fd) {
                error!("Failed to send fd to parent: {}", e);
                std::process::exit(1);
            }
            debug!("Child: sent notif_fd to parent");

            unsafe { libc::close(child_sock_fd) };
            unsafe { libc::close(notif_fd) };

            debug!("Child: about to exec {:?}", program);

            execvp(&program, &args)?;
            unreachable!();
        }
    }

    Ok(())
}

fn send_fd(sock: RawFd, fd: RawFd) -> Result<()> {
    use std::ptr;

    debug!("send_fd: sock={}, fd={}", sock, fd);

    let data = [1u8; 1];
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut _,
        iov_len: 1,
    };

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;

    if fd >= 0 {
        let cmsg_size = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
        let mut cmsg_buf = vec![0u8; cmsg_size];

        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut _;
        msg.msg_controllen = cmsg_size;

        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as usize;
            ptr::copy_nonoverlapping(
                &fd as *const _ as *const u8,
                libc::CMSG_DATA(cmsg),
                std::mem::size_of::<RawFd>(),
            );
        }

        let ret = unsafe { libc::sendmsg(sock, &msg, 0) };
        if ret < 0 {
            return Err(anyhow!(
                "sendmsg failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        debug!("send_fd: sendmsg returned {}", ret);
    } else {
        msg.msg_control = ptr::null_mut();
        msg.msg_controllen = 0;

        let ret = unsafe { libc::sendmsg(sock, &msg, 0) };
        if ret < 0 {
            return Err(anyhow!(
                "sendmsg failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        debug!("send_fd: sendmsg (no fd) returned {}", ret);
    }

    Ok(())
}

fn recv_fd(sock: RawFd) -> Result<RawFd> {
    use std::ptr;

    debug!("recv_fd: waiting on sock={}", sock);

    let mut data = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr() as *mut _,
        iov_len: 1,
    };

    let cmsg_size = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cmsg_buf = vec![0u8; cmsg_size];

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut _;
    msg.msg_controllen = cmsg_size;

    let ret = unsafe { libc::recvmsg(sock, &mut msg, 0) };
    if ret < 0 {
        return Err(anyhow!(
            "recvmsg failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    debug!("recv_fd: recvmsg returned {}, data[0]={}", ret, data[0]);

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if !cmsg.is_null() {
            debug!(
                "recv_fd: cmsg_level={}, cmsg_type={}, cmsg_len={}",
                (*cmsg).cmsg_level,
                (*cmsg).cmsg_type,
                (*cmsg).cmsg_len
            );

            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let mut fd: RawFd = -1;
                ptr::copy_nonoverlapping(
                    libc::CMSG_DATA(cmsg),
                    &mut fd as *mut _ as *mut u8,
                    std::mem::size_of::<RawFd>(),
                );
                debug!("recv_fd: extracted fd={}", fd);
                return Ok(fd);
            }
        } else {
            debug!("recv_fd: no control message");
        }
    }

    Ok(-1)
}

fn handle_notifications(child: Pid, notif_fd: RawFd) -> Result<()> {
    use std::sync::atomic::Ordering;

    NOTIF_FD.store(notif_fd, Ordering::SeqCst);

    info!("Starting notification handler loop for child {}", child);

    // Set notification fd to non-blocking for polling
    unsafe {
        let flags = libc::fcntl(notif_fd, libc::F_GETFL);
        libc::fcntl(notif_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }

    let mut poll_fd = libc::pollfd {
        fd: notif_fd,
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        // Poll with timeout to check child status periodically
        let poll_ret = unsafe { libc::poll(&mut poll_fd, 1, 100) }; // 100ms timeout

        if poll_ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            error!("poll failed: {}", err);
            break;
        }

        // Check child status
        match waitpid(child, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_, code)) => {
                info!("Child exited with code {}", code);
                return Ok(());
            }
            Ok(WaitStatus::Signaled(_, sig, _)) => {
                info!("Child killed by signal {:?}", sig);
                return Ok(());
            }
            Ok(WaitStatus::StillAlive) => {}
            Ok(other) => {
                debug!("Child status: {:?}", other);
            }
            Err(nix::errno::Errno::ECHILD) => {
                info!("Child no longer exists");
                return Ok(());
            }
            Err(e) => {
                warn!("waitpid error: {}", e);
            }
        }

        if poll_ret == 0 {
            // Timeout, no notifications pending
            continue;
        }

        if poll_fd.revents & libc::POLLIN == 0 {
            if poll_fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                debug!("Poll error/hangup on notification fd");
                break;
            }
            continue;
        }

        // Try to receive notification
        let notif = match notif_receive(notif_fd) {
            Ok(n) => n,
            Err(e) => {
                let err_str = e.to_string();

                if err_str.contains("Resource temporarily unavailable")
                    || err_str.contains("EAGAIN")
                    || err_str.contains("EWOULDBLOCK")
                {
                    // No notification available (non-blocking)
                    continue;
                }

                if err_str.contains("No such")
                    || err_str.contains("ENOENT")
                    || err_str.contains("Bad file")
                {
                    debug!("Notification receive ended: {}", e);
                    break;
                }

                error!("notif_receive error: {}", e);
                continue;
            }
        };

        let pid = Pid::from_raw(notif.pid as i32);

        trace!(
            "Syscall: pid={}, nr={} ({}), id={}",
            notif.pid,
            notif.data.nr,
            syscall_name(notif.data.nr),
            notif.id
        );

        CURRENT_NOTIF_ID.store(notif.id, Ordering::SeqCst);

        let result = handler::handle_syscall(pid, &notif.data);

        match result {
            SyscallResult::Response(resp) => {
                trace!(
                    "Response: val={}, error={}, flags={:#x}",
                    resp.val, resp.error, resp.flags
                );

                if let Err(e) = notif_respond(notif_fd, notif.id, resp.val, resp.error, resp.flags)
                {
                    let err_str = e.to_string();
                    if err_str.contains("No such") || err_str.contains("ENOENT") {
                        debug!("Process terminated before response");
                        continue;
                    }
                    warn!("Failed to respond: {}", e);
                }
            }
            SyscallResult::AlreadyHandled => {
                debug!("Syscall already handled via ADDFD");
            }
        }
    }

    info!("Waiting for child to exit...");
    match waitpid(child, None) {
        Ok(WaitStatus::Exited(_, code)) => {
            info!("Child exited with code {}", code);
        }
        Ok(WaitStatus::Signaled(_, sig, _)) => {
            info!("Child killed by signal {:?}", sig);
        }
        Ok(other) => {
            info!("Child wait status: {:?}", other);
        }
        Err(e) => {
            debug!("waitpid: {}", e);
        }
    }

    Ok(())
}

fn syscall_name(nr: i32) -> &'static str {
    match nr as i64 {
        libc::SYS_openat => "openat",
        libc::SYS_newfstatat => "newfstatat",
        libc::SYS_read => "read",
        libc::SYS_write => "write",
        libc::SYS_ioctl => "ioctl",
        libc::SYS_close => "close",
        libc::SYS_fstat => "fstat",
        libc::SYS_poll => "poll",
        libc::SYS_ppoll => "ppoll",
        libc::SYS_socket => "socket",
        libc::SYS_connect => "connect",
        libc::SYS_clone => "clone",
        libc::SYS_clone3 => "clone3",
        _ => "unknown",
    }
}

use std::sync::atomic::{AtomicI32, AtomicU64};

pub static NOTIF_FD: AtomicI32 = AtomicI32::new(-1);
pub static CURRENT_NOTIF_ID: AtomicU64 = AtomicU64::new(0);

pub fn get_notif_context() -> (RawFd, u64) {
    (
        NOTIF_FD.load(std::sync::atomic::Ordering::SeqCst),
        CURRENT_NOTIF_ID.load(std::sync::atomic::Ordering::SeqCst),
    )
}
