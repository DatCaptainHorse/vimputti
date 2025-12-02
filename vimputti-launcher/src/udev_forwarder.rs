use crate::state::{get_all_udev_broadcast_sockets, remove_udev_broadcast_socket};
use std::io::Read;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream;
use std::thread;
use tracing::*;

/// Start the udev event forwarder in a background thread
pub fn start_udev_forwarder() {
    thread::spawn(|| {
        loop {
            if let Err(e) = run_forwarder() {
                warn!("Udev forwarder error: {}, retrying in 1s...", e);
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    });
}

fn run_forwarder() -> std::io::Result<()> {
    let socket_path = "/tmp/vimputti/udev";

    info!("Connecting to udev event source at {}", socket_path);

    let mut stream = UnixStream::connect(socket_path)?;

    info!("Connected to udev event source");

    // Buffer for reading messages
    // The manager sends libudev wire format messages
    let mut buffer = vec![0u8; 8192];

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                warn!("Udev event source disconnected");
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "disconnected",
                ));
            }
            Ok(n) => {
                let message = &buffer[..n];
                debug!("Received {} byte udev event, forwarding to clients", n);

                // Log first few bytes for debugging
                if n >= 8 {
                    let prefix = String::from_utf8_lossy(&message[..8]);
                    trace!("Message prefix: {:?}", prefix);
                }

                broadcast_to_clients(message);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                continue;
            }
            Err(e) => {
                error!("Error reading from udev source: {}", e);
                return Err(e);
            }
        }
    }
}

fn broadcast_to_clients(message: &[u8]) {
    let sockets = get_all_udev_broadcast_sockets();

    if sockets.is_empty() {
        trace!("No udev clients to broadcast to");
        return;
    }

    debug!(
        "Broadcasting {} bytes to {} udev clients",
        message.len(),
        sockets.len()
    );

    for fd in sockets {
        let result = unsafe {
            libc::send(
                fd,
                message.as_ptr() as *const libc::c_void,
                message.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };

        if result < 0 {
            let err = std::io::Error::last_os_error();
            match err.kind() {
                std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::ConnectionReset => {
                    debug!("Removing dead udev datagram socket fd={}", fd);
                    remove_udev_broadcast_socket(fd);
                    unsafe { libc::close(fd) };
                }
                std::io::ErrorKind::WouldBlock => {
                    // Client not ready, skip
                    trace!("Udev client fd={} would block, skipping", fd);
                }
                _ => {
                    warn!("Failed to send to udev datagram fd={}: {}", fd, err);
                }
            }
        } else {
            trace!("Sent {} bytes to udev datagram fd={}", result, fd);
        }
    }
}
