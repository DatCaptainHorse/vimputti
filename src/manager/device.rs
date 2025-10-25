use crate::manager::sysfs::SysfsGenerator;
use crate::protocol::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace};

pub struct VirtualDevice {
    pub id: DeviceId,
    pub config: DeviceConfig,
    pub event_node: String,            // e.g., "event0"
    pub joystick_node: Option<String>, // e.g., "js0"
    socket_path: PathBuf,
    joystick_socket_path: Option<PathBuf>,
    base_path: PathBuf,
    clients: Arc<Mutex<Vec<tokio::net::unix::OwnedWriteHalf>>>,
    joystick_clients: Arc<Mutex<Vec<tokio::net::unix::OwnedWriteHalf>>>,
    feedback_clients: Arc<Mutex<Vec<UnixStream>>>,
    feedback_socket_path: Option<PathBuf>,
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
        let feedback_clients = Arc::new(Mutex::new(Vec::new()));

        // Start accepting client connections
        let clients_clone = clients.clone();
        let feedback_clients_clone = feedback_clients.clone();
        let config_clone = config.clone();
        let event_node_clone = event_node.clone();
        tokio::spawn(async move {
            Self::accept_clients(
                id,
                listener,
                clients_clone,
                feedback_clients_clone,
                config_clone,
                event_node_clone,
            )
            .await;
        });

        // Create feedback socket
        let feedback_socket_path = base_path
            .join("devices")
            .join(format!("{}.feedback", &event_node));
        let _ = std::fs::remove_file(&feedback_socket_path);

        let feedback_listener = UnixListener::bind(&feedback_socket_path)?;
        let feedback_clients_clone = Arc::clone(&feedback_clients);
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = feedback_listener.accept().await {
                    debug!("Client connected to feedback socket");
                    feedback_clients_clone.lock().await.push(stream);
                }
            }
        });

        // Create joystick interface if device has axes or buttons
        let (joystick_node, joystick_socket_path, joystick_clients) = if !config.buttons.is_empty()
            || !config.axes.is_empty()
        {
            let js_node = format!("js{}", id);
            let js_socket_path = base_path.join("devices").join(&js_node);

            // Remove old socket if exists
            let _ = std::fs::remove_file(&js_socket_path);

            // Create joystick socket
            let js_listener = UnixListener::bind(&js_socket_path)?;

            let js_clients = Arc::new(Mutex::new(Vec::new()));
            let js_clients_clone = js_clients.clone();
            let config_clone = config.clone();

            tokio::spawn(async move {
                Self::accept_joystick_clients(id, js_listener, js_clients_clone, config_clone).await;
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
            feedback_clients,
            feedback_socket_path: Some(feedback_socket_path),
        })
    }

    /// Accept client connections to device socket
    async fn accept_clients(
        id: DeviceId,
        listener: UnixListener,
        clients: Arc<Mutex<Vec<tokio::net::unix::OwnedWriteHalf>>>,
        feedback_clients: Arc<Mutex<Vec<UnixStream>>>,
        config: DeviceConfig,
        event_node: String,
    ) {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    debug!(
                        "Client connected to device socket: {} ({})",
                        event_node, config.name
                    );

                    let (mut read_half, mut write_half) = stream.into_split();

                    // Send handshake
                    let handshake = DeviceHandshake {
                        device_id: id,
                        config: config.clone(),
                    };
                    match serde_json::to_vec(&handshake) {
                        Ok(config_json) => {
                            let len = config_json.len() as u32;
                            if let Err(e) = write_half.write_all(&len.to_le_bytes()).await {
                                error!("Failed to send config length to client: {}", e);
                                continue;
                            }
                            if let Err(e) = write_half.write_all(&config_json).await {
                                error!("Failed to send config to client: {}", e);
                                continue;
                            }
                            debug!("Sent device config to client ({} bytes)", config_json.len());
                        }
                        Err(e) => {
                            error!("Failed to serialize device config: {}", e);
                            continue;
                        }
                    }

                    clients.lock().await.push(write_half);

                    // Spawn reader for feedback events
                    let feedback_clients = feedback_clients.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 24];
                        while read_half.read_exact(&mut buf).await.is_ok() {
                            let event: LinuxInputEvent =
                                unsafe { std::ptr::read(buf.as_ptr() as *const _) };

                            if event.event_type == EV_FF {
                                debug!(
                                    "Received feedback event: type={}, code={}, value={}",
                                    event.event_type, event.code, event.value
                                );
                                let mut clients = feedback_clients.lock().await;
                                debug!("Writing to {} feedback clients", clients.len());
                                let mut disconnected = Vec::new();

                                for (idx, client) in clients.iter_mut().enumerate() {
                                    if let Err(e) = client.write_all(&buf).await {
                                        trace!("Failed to write to feedback client {}: {}", idx, e);
                                        disconnected.push(idx);
                                    } else {
                                        debug!("Wrote feedback to client {}", idx);
                                    }
                                }

                                // Remove disconnected clients in reverse order
                                for idx in disconnected.iter().rev() {
                                    clients.remove(*idx);
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Error accepting client: {}", e);
                    break;
                }
            }
        }
    }

    async fn accept_joystick_clients(
        id: DeviceId,
        listener: UnixListener,
        clients: Arc<Mutex<Vec<tokio::net::unix::OwnedWriteHalf>>>,
        config: DeviceConfig,
    ) {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    info!("Client connected to joystick socket");

                    let (_, mut write_half) = stream.into_split();

                    // Send handshake
                    let handshake = DeviceHandshake {
                        device_id: id,
                        config: config.clone(),
                    };
                    match serde_json::to_vec(&handshake) {
                        Ok(config_json) => {
                            let len = config_json.len() as u32;
                            if write_half.write_all(&len.to_le_bytes()).await.is_err()
                                || write_half.write_all(&config_json).await.is_err()
                            {
                                continue;
                            }
                        }
                        Err(_) => continue,
                    }

                    clients.lock().await.push(write_half);
                }
                Err(e) => {
                    error!("Error accepting joystick client: {}", e);
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
        let mut has_sync = false;
        let mut linux_events: Vec<LinuxInputEvent> =
            events.iter().map(|e| e.to_linux_input_event()).collect();

        for event in &linux_events {
            if event.event_type == EV_SYN && event.code == SYN_REPORT {
                has_sync = true;
                break;
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
            match client.write_all(&data).await {
                Ok(()) => {
                    // Success
                }
                Err(e) => {
                    trace!("Failed to write to evdev client {}: {}", idx, e);
                    disconnected.push(idx);
                }
            }
        }

        // Remove disconnected/slow clients (in reverse order)
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
                    // Find button index in config
                    if let Some(button_idx) = self.config.buttons.iter().position(|b| b == button) {
                        js_events.push(LinuxJsEvent {
                            time,
                            value: if *pressed { 1 } else { 0 },
                            type_: JS_EVENT_BUTTON,
                            number: button_idx as u8,
                        });
                    }
                }
                InputEvent::Axis { axis, value } => {
                    if let Some(axis_idx) = self.config.axes.iter().position(|a| a.axis == *axis) {
                        // Clamp the i32 value to i16 range BEFORE casting
                        let clamped_value = value.clamp(&(i16::MIN as i32), &(i16::MAX as i32));
                        let normalized_value = *clamped_value as i16;
                        js_events.push(LinuxJsEvent {
                            time,
                            value: normalized_value,
                            type_: JS_EVENT_AXIS,
                            number: axis_idx as u8,
                        });
                    }
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
            match client.write_all(&data).await {
                Ok(()) => {
                    // Success
                }
                Err(e) => {
                    trace!("Failed to write to joystick client {}: {}", idx, e);
                    disconnected.push(idx);
                }
            }
        }

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

        // Clean up feedback socket
        if let Some(feedback_socket) = &self.feedback_socket_path {
            let _ = std::fs::remove_file(feedback_socket);
        }

        // Clean up sysfs files
        let _ = SysfsGenerator::remove_device_files(self.id, &self.base_path);

        info!("Device {} cleaned up", self.event_node);
    }
}
