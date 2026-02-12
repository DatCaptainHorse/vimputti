use crate::protocol::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

mod device;
mod lock;

pub use device::VirtualDevice;
pub use lock::LockFile;

pub struct Manager {
    /// Base directory for all vimputti files
    base_path: PathBuf,
    /// Socket path for control commands
    control_socket_path: PathBuf,
    /// Lock file to prevent multiple managers with same instance
    _lock_file: LockFile,
    /// Registry of active virtual devices
    devices: Arc<Mutex<HashMap<DeviceId, Arc<VirtualDevice>>>>,
    /// Next device ID to assign
    next_device_id: Arc<Mutex<DeviceId>>,
    /// Pool of device IDs available for reuse
    free_device_ids: Arc<Mutex<Vec<DeviceId>>>,
}

impl Manager {
    /// Create a new manager instance
    pub fn new(socket_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let socket_path = socket_path.as_ref();
        let base_path = std::env::var("VIMPUTTI_BASE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| socket_path.parent().unwrap().join("vimputti"));

        // Create base directory
        std::fs::create_dir_all(&base_path)?;

        // Acquire lock file
        let lock_path = socket_path.with_extension("lock");
        let lock_file = LockFile::acquire(&lock_path)?;

        info!("Manager initialized at {}", socket_path.display());

        Ok(Self {
            base_path,
            control_socket_path: socket_path.to_path_buf(),
            _lock_file: lock_file,
            next_device_id: Arc::new(Mutex::new(0)),
            free_device_ids: Arc::new(Mutex::new(Vec::new())),
            devices: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Run the manager main loop
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Remove existing socket if present
        let _ = std::fs::remove_file(&self.control_socket_path);

        // Bind control socket
        let listener = UnixListener::bind(&self.control_socket_path)?;

        // Set socket permissions
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

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let devices = self.devices.clone();
                    let next_device_id = self.next_device_id.clone();
                    let free_device_ids = self.free_device_ids.clone();
                    let base_path = self.base_path.clone();

                    tokio::spawn(async move {
                        if let Err(e) =
                            Self::handle_client(stream, devices, next_device_id, free_device_ids)
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
        devices: Arc<Mutex<HashMap<DeviceId, Arc<VirtualDevice>>>>,
        next_device_id: Arc<Mutex<DeviceId>>,
        free_device_ids: Arc<Mutex<Vec<DeviceId>>>,
    ) -> anyhow::Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let message: ControlMessage = match serde_json::from_str(&line) {
                        Ok(msg) => msg,
                        Err(e) => {
                            warn!("Failed to parse message: {}", e);
                            continue;
                        }
                    };

                    trace!("Received command: {:?}", message.command);

                    let response = Self::process_command(
                        message.command,
                        &devices,
                        &next_device_id,
                        &free_device_ids,
                    )
                    .await;

                    let response = ControlResponse {
                        id: message.id,
                        result: response,
                    };

                    let response_json = serde_json::to_string(&response)?;

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
        devices: &Arc<Mutex<HashMap<DeviceId, Arc<VirtualDevice>>>>,
        next_device_id: &Arc<Mutex<DeviceId>>,
        free_device_ids: &Arc<Mutex<Vec<DeviceId>>>,
    ) -> ControlResult {
        match command {
            ControlCommand::CreateDevice { config } => {
                // Get device ID
                let device_id = {
                    let mut free_ids = free_device_ids.lock().await;
                    if let Some(id) = free_ids.pop() {
                        debug!("Re-using device ID: {}", id);
                        id
                    } else {
                        let mut next_id = next_device_id.lock().await;
                        let id = *next_id;
                        *next_id += 1;
                        debug!("Using next device ID: {}", id);
                        id
                    }
                };

                debug!(
                    "Creating device {} with config: name={}, vendor_id=0x{:04x}, product_id=0x{:04x}",
                    device_id, config.name, config.vendor_id, config.product_id
                );

                // Create device (blocking operation, run in spawn_blocking)
                let config_clone = config.clone();

                match tokio::task::spawn_blocking(move || {
                    VirtualDevice::create(device_id, config_clone)
                })
                .await
                {
                    Ok(Ok(device)) => {
                        let event_node = device.event_node.clone();
                        devices.lock().await.insert(device_id, Arc::new(device));

                        info!("Created device {} as {}", device_id, event_node);

                        ControlResult::DeviceCreated {
                            device_id,
                            event_node,
                        }
                    }
                    Ok(Err(e)) => ControlResult::Error {
                        message: format!("Failed to create device: {}", e),
                    },
                    Err(e) => ControlResult::Error {
                        message: format!("Task join error: {}", e),
                    },
                }
            }
            ControlCommand::DestroyDevice { device_id } => {
                let device = devices.lock().await.remove(&device_id);
                match device {
                    Some(_device) => {
                        info!("Destroyed device {}", device_id);
                        free_device_ids.lock().await.push(device_id);
                        debug!("Marking device ID {} as re-usable", device_id);
                        ControlResult::DeviceDestroyed
                    }
                    None => ControlResult::Error {
                        message: format!("Device {} not found", device_id),
                    },
                }
            }
            ControlCommand::SendInput { device_id, events } => {
                let device = {
                    let devices = devices.lock().await;
                    devices.get(&device_id).cloned()
                };

                match device {
                    Some(device) => {
                        // Send events (blocking call, use spawn_blocking)
                        match tokio::task::spawn_blocking(move || device.send_events(&events)).await
                        {
                            Ok(Ok(())) => ControlResult::InputSent,
                            Ok(Err(e)) => ControlResult::Error {
                                message: format!("Failed to send input: {}", e),
                            },
                            Err(e) => ControlResult::Error {
                                message: format!("Task join error: {}", e),
                            },
                        }
                    }
                    None => ControlResult::Error {
                        message: format!("Device {} not found", device_id),
                    },
                }
            }
            ControlCommand::PollFeedback { device_id } => {
                debug!("Polling feedback for device {}", device_id);

                let devices_lock = devices.lock().await;
                if let Some(device) = devices_lock.get(&device_id) {
                    let device_clone = Arc::clone(device);
                    drop(devices_lock);

                    // Try to read feedback in blocking task (non-blocking read with timeout)
                    match tokio::task::spawn_blocking(move || device_clone.read_feedback_event())
                        .await
                    {
                        Ok(Ok(event)) => {
                            debug!("Feedback event polled: {:?}", event);
                            ControlResult::FeedbackPolled { event: Some(event) }
                        }
                        Ok(Err(_)) => {
                            // No feedback available (EAGAIN or other read error)
                            ControlResult::FeedbackPolled { event: None }
                        }
                        Err(e) => ControlResult::Error {
                            message: format!("Failed to poll feedback: {}", e),
                        },
                    }
                } else {
                    ControlResult::Error {
                        message: format!("Device {} not found", device_id),
                    }
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
