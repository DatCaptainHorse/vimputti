use crate::manager::sysfs::SysfsGenerator;
use crate::protocol::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

pub struct VirtualDevice {
    pub id: DeviceId,
    pub config: DeviceConfig,
    pub event_node: String, // e.g., "event0"
    socket_path: PathBuf,
    base_path: PathBuf,
    dev_input_symlink: Option<PathBuf>, // symlink in /dev/input
    clients: Arc<Mutex<Vec<UnixStream>>>,
}

impl VirtualDevice {
    /// Create a new virtual device
    pub async fn create(
        id: DeviceId,
        config: DeviceConfig,
        base_path: &Path,
    ) -> anyhow::Result<Self> {
        let event_node = format!("event{}", id);
        let socket_path = base_path.join("devices").join(&event_node);

        // Remove old socket if exists
        let _ = std::fs::remove_file(&socket_path);

        // Create device socket
        let listener = UnixListener::bind(&socket_path)?;

        // Create sysfs entries using new generator
        SysfsGenerator::create_device_files(id, &config, base_path)?;

        let clients = Arc::new(Mutex::new(Vec::new()));

        // Start accepting client connections
        let clients_clone = Arc::clone(&clients);
        tokio::spawn(async move {
            Self::accept_clients(listener, clients_clone).await;
        });

        // Try to create symlink in /dev/input
        let dev_input_symlink = Self::try_create_dev_symlink(&event_node, &socket_path);

        Ok(Self {
            id,
            config,
            event_node,
            socket_path,
            base_path: base_path.to_path_buf(),
            dev_input_symlink,
            clients,
        })
    }

    /// Try to create a symlink in /dev/input pointing to our socket
    fn try_create_dev_symlink(event_node: &str, socket_path: &Path) -> Option<PathBuf> {
        let symlink_path = PathBuf::from("/dev/input").join(event_node);

        match std::os::unix::fs::symlink(socket_path, &symlink_path) {
            Ok(_) => {
                info!(
                    "Created symlink: {} -> {}",
                    symlink_path.display(),
                    socket_path.display()
                );
                Some(symlink_path)
            }
            Err(e) => {
                warn!(
                    "Failed to create symlink in /dev/input (this is OK, using shim fallback): {}",
                    e
                );
                None
            }
        }
    }

    /// Accept client connections to device socket
    async fn accept_clients(listener: UnixListener, clients: Arc<Mutex<Vec<UnixStream>>>) {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    info!("Client connected to device socket");
                    clients.lock().await.push(stream);
                }
                Err(e) => {
                    error!("Error accepting client: {}", e);
                    break;
                }
            }
        }
    }

    /// Send input events to all connected clients
    pub async fn send_events(&self, events: &[InputEvent]) -> anyhow::Result<()> {
        let mut linux_events = Vec::new();
        let mut has_sync = false;

        // Convert high-level events to Linux input events
        for event in events {
            match event {
                InputEvent::Button { button, pressed } => {
                    linux_events.push(LinuxInputEvent::new(
                        EV_KEY,
                        button.to_code(),
                        if *pressed { 1 } else { 0 },
                    ));
                }
                InputEvent::Axis { axis, value } => {
                    linux_events.push(LinuxInputEvent::new(EV_ABS, axis.to_code(), *value));
                }
                InputEvent::Raw {
                    event_type,
                    code,
                    value,
                } => {
                    linux_events.push(LinuxInputEvent::new(*event_type, *code, *value));
                }
                InputEvent::Sync => {
                    has_sync = true;
                    linux_events.push(LinuxInputEvent::new(EV_SYN, SYN_REPORT, 0));
                }
            }
        }

        // Add SYN_REPORT if not present
        if !has_sync && !linux_events.is_empty() {
            linux_events.push(LinuxInputEvent::new(EV_SYN, SYN_REPORT, 0));
        }

        // Convert to bytes
        let mut data = Vec::new();
        for event in &linux_events {
            data.extend_from_slice(&event.to_bytes());
        }

        // Send to all connected clients
        let mut clients = self.clients.lock().await;
        let mut disconnected = Vec::new();

        for (idx, client) in clients.iter_mut().enumerate() {
            if let Err(e) = client.write_all(&data).await {
                error!("Failed to write to client: {}", e);
                disconnected.push(idx);
            }
        }

        // Remove disconnected clients (in reverse order to maintain indices)
        for idx in disconnected.iter().rev() {
            clients.remove(*idx);
        }

        Ok(())
    }
}

impl Drop for VirtualDevice {
    fn drop(&mut self) {
        // Clean up socket file
        let _ = std::fs::remove_file(&self.socket_path);

        // Clean up symlink if it exists
        if let Some(symlink) = &self.dev_input_symlink {
            let _ = std::fs::remove_file(symlink);
            info!("Removed symlink: {}", symlink.display());
        }

        // Clean up sysfs files
        let _ = SysfsGenerator::remove_device_files(self.id, &self.base_path);

        info!("Device {} cleaned up", self.event_node);
    }
}
