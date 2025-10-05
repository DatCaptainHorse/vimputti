use crate::protocol::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

mod device;
mod lock;
mod sysfs;
mod udev;

pub use device::VirtualDevice;
pub use lock::LockFile;
pub use sysfs::SysfsGenerator;
pub use udev::UdevBroadcaster;

pub struct Manager {
    /// Base directory for all vimputti files
    base_path: PathBuf,
    /// Socket path for control commands
    control_socket_path: PathBuf,
    /// Lock file to prevent multiple managers with same instance
    _lock_file: LockFile,
    /// Registry of active virtual devices
    devices: Arc<Mutex<HashMap<DeviceId, VirtualDevice>>>,
    /// Next device ID to assign
    next_device_id: Arc<Mutex<DeviceId>>,
    /// Udev event broadcaster
    udev_broadcaster: Arc<UdevBroadcaster>,
}

impl Manager {
    /// Create a new manager instance
    pub fn new(socket_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let socket_path = socket_path.as_ref();
        let base_path = socket_path.parent().unwrap().join("vimputti");

        // Create base directory structure
        std::fs::create_dir_all(&base_path)?;
        std::fs::create_dir_all(base_path.join("devices"))?;
        std::fs::create_dir_all(base_path.join("sysfs/class/input"))?;
        std::fs::create_dir_all(base_path.join("sysfs/devices/virtual/input"))?;

        // Acquire lock file
        let lock_path = socket_path.with_extension("lock");
        let lock_file = LockFile::acquire(&lock_path)?;

        // Create udev broadcaster
        let udev_broadcaster = Arc::new(UdevBroadcaster::new(&base_path)?);

        info!("Manager initialized at {}", socket_path.display());

        Ok(Self {
            base_path,
            control_socket_path: socket_path.to_path_buf(),
            _lock_file: lock_file,
            devices: Arc::new(Mutex::new(HashMap::new())),
            next_device_id: Arc::new(Mutex::new(0)),
            udev_broadcaster,
        })
    }

    /// Run the manager main loop
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Remove existing socket if present
        let _ = std::fs::remove_file(&self.control_socket_path);

        // Bind control socket
        let listener = UnixListener::bind(&self.control_socket_path)?;

        // Set socket permissions to allow all users in container
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &self.control_socket_path,
                std::fs::Permissions::from_mode(0o666),
            )?;
        }

        info!(
            "Manager listening on {}",
            self.control_socket_path.display()
        );

        // Start udev broadcaster
        let udev_broadcaster = Arc::clone(&self.udev_broadcaster);
        tokio::spawn(async move {
            udev_broadcaster.run().await;
        });

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let devices = Arc::clone(&self.devices);
                    let next_device_id = Arc::clone(&self.next_device_id);
                    let base_path = self.base_path.clone();
                    let udev_broadcaster = Arc::clone(&self.udev_broadcaster);

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_client(
                            stream,
                            devices,
                            next_device_id,
                            base_path,
                            udev_broadcaster,
                        )
                        .await
                        {
                            error!("Client handler error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    /// Handle a single client connection
    async fn handle_client(
        stream: UnixStream,
        devices: Arc<Mutex<HashMap<DeviceId, VirtualDevice>>>,
        next_device_id: Arc<Mutex<DeviceId>>,
        base_path: PathBuf,
        udev_broadcaster: Arc<UdevBroadcaster>,
    ) -> anyhow::Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    // Connection closed cleanly
                    break;
                }
                Ok(_) => {
                    let message: ControlMessage = match serde_json::from_str(&line) {
                        Ok(msg) => msg,
                        Err(e) => {
                            warn!("Failed to parse message: {}", e);
                            continue;
                        }
                    };

                    info!("Received command: {:?}", message.command);

                    let response = Self::process_command(
                        message.command,
                        &devices,
                        &next_device_id,
                        &base_path,
                        &udev_broadcaster,
                    )
                    .await;

                    let response = ControlResponse {
                        id: message.id,
                        result: response,
                    };

                    let response_json = serde_json::to_string(&response)?;

                    // Try to write response, but don't error on broken pipe
                    if let Err(e) = writer.write_all(response_json.as_bytes()).await {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            break;
                        }
                        return Err(e.into());
                    }
                    if let Err(e) = writer.write_all(b"\n").await {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            break;
                        }
                        return Err(e.into());
                    }
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::UnexpectedEof {
                        break;
                    }
                    error!("Error reading from client: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    /// Process a control command
    async fn process_command(
        command: ControlCommand,
        devices: &Arc<Mutex<HashMap<DeviceId, VirtualDevice>>>,
        next_device_id: &Arc<Mutex<DeviceId>>,
        base_path: &Path,
        udev_broadcaster: &Arc<UdevBroadcaster>,
    ) -> ControlResult {
        match command {
            ControlCommand::CreateDevice { config } => {
                let device_id = {
                    let mut next_id = next_device_id.lock().await;
                    let id = *next_id;
                    *next_id += 1;
                    id
                };

                match VirtualDevice::create(device_id, config.clone(), base_path).await {
                    Ok(device) => {
                        let event_node = device.event_node.clone();
                        devices.lock().await.insert(device_id, device);

                        info!("Created device {} as {}", device_id, event_node);

                        // Broadcast udev add event (after device is ready)
                        if let Err(e) = udev_broadcaster.broadcast_add(device_id, &config) {
                            warn!("Failed to broadcast udev add event: {}", e);
                        }

                        ControlResult::DeviceCreated {
                            device_id,
                            event_node,
                        }
                    }
                    Err(e) => ControlResult::Error {
                        message: format!("Failed to create device: {}", e),
                    },
                }
            }

            ControlCommand::DestroyDevice { device_id } => {
                let device = devices.lock().await.remove(&device_id);
                match device {
                    Some(device) => {
                        info!("Destroyed device {}", device_id);

                        // Broadcast udev remove event
                        if let Err(e) = udev_broadcaster.broadcast_remove(device_id, &device.config)
                        {
                            warn!("Failed to broadcast udev remove event: {}", e);
                        }

                        ControlResult::DeviceDestroyed
                    }
                    None => ControlResult::Error {
                        message: format!("Device {} not found", device_id),
                    },
                }
            }

            ControlCommand::SendInput { device_id, events } => {
                let devices = devices.lock().await;
                match devices.get(&device_id) {
                    Some(device) => match device.send_events(&events).await {
                        Ok(_) => ControlResult::InputSent,
                        Err(e) => ControlResult::Error {
                            message: format!("Failed to send input: {}", e),
                        },
                    },
                    None => ControlResult::Error {
                        message: format!("Device {} not found", device_id),
                    },
                }
            }

            ControlCommand::ListDevices => {
                let devices = devices.lock().await;
                let device_list: Vec<DeviceInfo> = devices
                    .values()
                    .map(|d| DeviceInfo {
                        device_id: d.id,
                        name: d.config.name.clone(),
                        event_node: d.event_node.clone(),
                        joystick_node: d.joystick_node.clone(),
                        vendor_id: d.config.vendor_id,
                        product_id: d.config.product_id,
                    })
                    .collect();
                ControlResult::DeviceList(device_list)
            }

            ControlCommand::Ping => ControlResult::Pong,
        }
    }
}
