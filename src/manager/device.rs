use crate::manager::sysfs::SysfsGenerator;
use crate::protocol::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{error, info};

pub struct VirtualDevice {
    pub id: DeviceId,
    pub config: DeviceConfig,
    pub event_node: String,            // e.g., "event0"
    pub joystick_node: Option<String>, // e.g., "js0"
    socket_path: PathBuf,
    joystick_socket_path: Option<PathBuf>,
    base_path: PathBuf,
    clients: Arc<Mutex<Vec<UnixStream>>>,
    joystick_clients: Arc<Mutex<Vec<UnixStream>>>,
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
        let config_clone = config.clone();
        tokio::spawn(async move {
            Self::accept_clients(listener, clients_clone, config_clone).await;
        });

        // Create joystick interface if device has axes or buttons
        let (joystick_node, joystick_socket_path, joystick_clients) =
            if !config.buttons.is_empty() || !config.axes.is_empty() {
                let js_node = format!("js{}", id);
                let js_socket_path = base_path.join("devices").join(&js_node);

                // Remove old socket if exists
                let _ = std::fs::remove_file(&js_socket_path);

                // Create joystick socket
                let js_listener = UnixListener::bind(&js_socket_path)?;

                let js_clients = Arc::new(Mutex::new(Vec::new()));
                let js_clients_clone = Arc::clone(&js_clients);
                let config_clone = config.clone();

                tokio::spawn(async move {
                    Self::accept_clients(js_listener, js_clients_clone, config_clone).await;
                });

                info!("Created joystick node: {}", js_node);

                (Some(js_node), Some(js_socket_path), js_clients)
            } else {
                (None, None, Arc::new(Mutex::new(Vec::new())))
            };

        Ok(Self {
            id,
            config,
            event_node,
            joystick_node,
            socket_path,
            joystick_socket_path,
            base_path: base_path.to_path_buf(),
            clients,
            joystick_clients,
        })
    }

    /// Accept client connections to device socket
    async fn accept_clients(
        listener: UnixListener,
        clients: Arc<Mutex<Vec<UnixStream>>>,
        config: DeviceConfig,
    ) {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    info!("Client connected to device socket");

                    // Send device config to the client as the first message
                    // Format: 4-byte length prefix + JSON config
                    match serde_json::to_vec(&config) {
                        Ok(config_json) => {
                            let len = config_json.len() as u32;
                            if let Err(e) = stream.write_all(&len.to_le_bytes()).await {
                                error!("Failed to send config length to client: {}", e);
                                continue;
                            }
                            if let Err(e) = stream.write_all(&config_json).await {
                                error!("Failed to send config to client: {}", e);
                                continue;
                            }
                            info!("Sent device config to client ({} bytes)", config_json.len());
                        }
                        Err(e) => {
                            error!("Failed to serialize device config: {}", e);
                            continue;
                        }
                    }

                    clients.lock().await.push(stream);
                }
                Err(e) => {
                    error!("Error accepting client: {}", e);
                    break;
                }
            }
        }
    }

    /// Send input events to all connected clients (both evdev and joystick)
    pub async fn send_events(&self, events: &[InputEvent]) -> anyhow::Result<()> {
        // Send to evdev clients
        self.send_evdev_events(events).await?;

        // Send to joystick clients
        self.send_joystick_events(events).await?;

        Ok(())
    }

    /// Send evdev events
    async fn send_evdev_events(&self, events: &[InputEvent]) -> anyhow::Result<()> {
        let mut linux_events = Vec::new();
        let mut has_sync = false;

        // Convert high-level events to Linux input events
        for event in events {
            match event {
                InputEvent::Button { button, pressed } => {
                    linux_events.push(LinuxInputEvent::new(
                        EV_KEY,
                        button.to_ev_code(),
                        if *pressed { 1 } else { 0 },
                    ));
                }
                InputEvent::Axis { axis, value } => {
                    linux_events.push(LinuxInputEvent::new(EV_ABS, axis.to_ev_code(), *value));
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

        // Send to all connected evdev clients
        let mut clients = self.clients.lock().await;
        let mut disconnected = Vec::new();

        for (idx, client) in clients.iter_mut().enumerate() {
            if let Err(e) = client.write_all(&data).await {
                error!("Failed to write to evdev client: {}", e);
                disconnected.push(idx);
            }
        }

        // Remove disconnected clients (in reverse order to maintain indices)
        for idx in disconnected.iter().rev() {
            clients.remove(*idx);
        }

        Ok(())
    }

    /// Send joystick events
    async fn send_joystick_events(&self, events: &[InputEvent]) -> anyhow::Result<()> {
        if self.joystick_node.is_none() {
            return Ok(());
        }

        const JS_EVENT_BUTTON: u8 = 0x01;
        const JS_EVENT_AXIS: u8 = 0x02;

        let mut js_events = Vec::new();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as u32;

        for event in events {
            match event {
                InputEvent::Button { button, pressed } => {
                    js_events.push(LinuxJsEvent {
                        time,
                        value: if *pressed { 1 } else { 0 },
                        type_: JS_EVENT_BUTTON,
                        number: button.to_ev_code() as u8, //to_js_code
                    });
                }
                InputEvent::Axis { axis, value } => {
                    // Normalize value to i16 range
                    let normalized_value = (*value as i16).clamp(i16::MIN, i16::MAX);
                    js_events.push(LinuxJsEvent {
                        time,
                        value: normalized_value,
                        type_: JS_EVENT_AXIS,
                        number: axis.to_ev_code() as u8, //to_js_code
                    });
                }
                _ => {} // Ignore raw events and sync for joystick
            }
        }

        // Convert to bytes - manually serialize to ensure correct layout
        let mut data = Vec::with_capacity(js_events.len() * 8);
        for event in &js_events {
            data.extend_from_slice(&event.time.to_ne_bytes());
            data.extend_from_slice(&event.value.to_ne_bytes());
            data.push(event.type_);
            data.push(event.number);
        }

        // Send to all connected joystick clients
        let mut clients = self.joystick_clients.lock().await;
        let mut disconnected = Vec::new();

        for (idx, client) in clients.iter_mut().enumerate() {
            if let Err(e) = client.write_all(&data).await {
                error!("Failed to write to joystick client: {}", e);
                disconnected.push(idx);
            }
        }

        // Remove disconnected clients
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

        // Clean up joystick socket
        if let Some(js_socket) = &self.joystick_socket_path {
            let _ = std::fs::remove_file(js_socket);
        }

        // Clean up sysfs files
        let _ = SysfsGenerator::remove_device_files(self.id, &self.base_path);

        info!("Device {} cleaned up", self.event_node);
    }
}
