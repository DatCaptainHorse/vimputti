use crate::handler::SyscallResult;
use crate::seccomp::SeccompData;
use crate::seccomp::SeccompNotifResp;
use crate::state::{
    is_netlink_socket, register_udev_broadcast_socket, register_udev_socket, track_netlink_socket,
};
use nix::unistd::Pid;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixDatagram;
use tracing::*;

// Socket constants
const AF_NETLINK: i32 = 16;
const NETLINK_KOBJECT_UEVENT: i32 = 15;

pub fn handle_socket(pid: Pid, data: &SeccompData) -> SyscallResult {
    let domain = data.args[0] as i32;
    let sock_type = data.args[1] as i32;
    let protocol = data.args[2] as i32;

    // Only intercept netlink kobject uevent sockets
    if domain == AF_NETLINK && protocol == NETLINK_KOBJECT_UEVENT {
        debug!(
            "Intercepting netlink udev socket: domain={}, type={}, protocol={}",
            domain, sock_type, protocol
        );
        return create_udev_socket_replacement(pid, sock_type);
    }

    // Let ALL other sockets through to kernel
    SyscallResult::Response(SeccompNotifResp::new_continue())
}

pub fn handle_bind(pid: Pid, data: &SeccompData) -> SyscallResult {
    let fd = data.args[0] as i32;

    // Only fake bind for our tracked netlink sockets
    if is_netlink_socket(pid, fd) {
        debug!("Faking bind() on udev socket fd {}", fd);
        return SyscallResult::Response(SeccompNotifResp::new_success(0));
    }

    // Let ALL other binds through to kernel
    SyscallResult::Response(SeccompNotifResp::new_continue())
}

fn create_udev_socket_replacement(pid: Pid, sock_type: i32) -> SyscallResult {
    // Create a Unix datagram socket pair
    // One end goes to the target process, we keep the other to send events
    let (our_socket, their_socket) = match UnixDatagram::pair() {
        Ok(pair) => pair,
        Err(e) => {
            warn!("Failed to create Unix datagram pair: {}", e);
            return SyscallResult::Response(SeccompNotifResp::new_continue());
        }
    };

    // Check if SOCK_NONBLOCK was requested (0x800)
    let nonblock = (sock_type & 0x800) != 0;
    if nonblock {
        if let Err(e) = their_socket.set_nonblocking(true) {
            warn!("Failed to set nonblocking on their socket: {}", e);
        }
    }
    // Our end should always be non-blocking for the event loop
    if let Err(e) = our_socket.set_nonblocking(true) {
        warn!("Failed to set nonblocking on our socket: {}", e);
    }

    let their_fd = their_socket.as_raw_fd();

    // Inject their end into the target process
    let target_fd = match crate::handler::inject_fd(their_fd) {
        Ok(fd) => fd,
        Err(e) => {
            error!("Failed to inject udev socket fd: {}", e);
            return SyscallResult::Response(SeccompNotifResp::new_continue());
        }
    };

    // Don't close the sockets when they go out of scope
    std::mem::forget(their_socket);

    let our_fd = our_socket.as_raw_fd();
    std::mem::forget(our_socket);

    // Track this socket in per-process state (for bind interception)
    track_netlink_socket(pid, target_fd);
    register_udev_socket(pid, target_fd, our_fd);

    // Also register globally for broadcasting
    register_udev_broadcast_socket(our_fd);

    info!(
        "Replaced netlink udev socket with Unix datagram pair, target_fd={}, our_fd={}",
        target_fd, our_fd
    );
    SyscallResult::Response(SeccompNotifResp::new_success(target_fd as i64))
}
