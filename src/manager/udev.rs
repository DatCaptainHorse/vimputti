use crate::protocol::*;
use anyhow::Result;
use std::path::Path;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
use tracing::{debug, error, info};

/// Udev event broadcaster
pub struct UdevBroadcaster {
    listener: UnixListener,
    event_tx: broadcast::Sender<UdevEvent>,
}

/// A udev event (hotplug notification)
#[derive(Debug, Clone)]
pub struct UdevEvent {
    pub action: UdevAction,
    pub device_info: UdevDeviceInfo,
}

#[derive(Debug, Clone)]
pub enum UdevAction {
    Add,
    Remove,
    Change,
}

#[derive(Debug, Clone)]
pub struct UdevDeviceInfo {
    pub subsystem: String,
    pub devtype: String,
    pub devname: String,
    pub devpath: String,
    pub syspath: String,
    pub properties: Vec<(String, String)>,
}

impl UdevBroadcaster {
    /// Create a new udev broadcaster
    pub fn new(base_path: &Path) -> Result<Self> {
        let socket_path = base_path.join("udev");

        // Remove old socket if exists
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path)?;

        info!("Udev socket created at {}", socket_path.display());

        // Create broadcast channel for events
        let (event_tx, _) = broadcast::channel(100);

        Ok(Self { listener, event_tx })
    }

    /// Start accepting udev monitor connections
    pub async fn run(&self) {
        let listener = &self.listener;
        let event_tx = self.event_tx.clone();

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("udev monitor connected from {:?}", addr);

                    // Check if socket is actually connected
                    if let Ok(peer) = stream.peer_addr() {
                        info!("Peer address: {:?}", peer);
                    }

                    let mut event_rx = event_tx.subscribe();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_monitor(stream, &mut event_rx).await {
                            debug!("udev monitor disconnected: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept udev connection: {}", e);
                }
            }
        }
    }

    /// Handle a single udev monitor connection
    async fn handle_monitor(
        mut stream: UnixStream,
        event_rx: &mut broadcast::Receiver<UdevEvent>,
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        let mut pending_messages: Vec<String> = Vec::new();

        loop {
            tokio::select! {
                // Receive new events from broadcast
                event_result = event_rx.recv() => {
                    match event_result {
                        Ok(event) => {
                            let message = Self::format_udev_message(&event);
                            debug!("Queued udev event ({} bytes)", message.len());
                            pending_messages.push(message);
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            debug!("Udev monitor lagged, skipped {} events", skipped);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(anyhow::anyhow!("Event channel closed"));
                        }
                    }
                }

                // Try to write pending messages when socket is writable
                _ = stream.writable(), if !pending_messages.is_empty() => {
                    // Try to write all pending messages
                    let mut written_count = 0;

                    for message in &pending_messages {
                        match tokio::time::timeout(
                            std::time::Duration::from_millis(10),
                            stream.write_all(message.as_bytes())
                        ).await {
                            Ok(Ok(())) => {
                                written_count += 1;
                            }
                            Ok(Err(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                // Socket buffer full, try again later
                                break;
                            }
                            Ok(Err(e)) => {
                                debug!("Udev write error: {}", e);
                                return Err(e.into());
                            }
                            Err(_) => {
                                // Timeout, socket might be blocked
                                break;
                            }
                        }
                    }

                    // Remove written messages
                    pending_messages.drain(..written_count);

                    if written_count > 0 {
                        debug!("Sent {} udev events", written_count);
                        let _ = stream.flush().await;
                    }

                    // Drop connection if queue grows too large (client not reading)
                    if pending_messages.len() > 50 {
                        debug!("Udev monitor queue overflow, dropping connection");
                        return Err(anyhow::anyhow!("Queue overflow"));
                    }
                }
            }
        }
    }

    /// Format a udev event as a netlink-style message
    fn format_udev_message(event: &UdevEvent) -> String {
        let action = match event.action {
            UdevAction::Add => "add",
            UdevAction::Remove => "remove",
            UdevAction::Change => "change",
        };

        let mut msg = format!(
            "ACTION={}\n\
             DEVNAME={}\n\
             DEVPATH={}\n\
             SUBSYSTEM={}\n",
            action,
            event.device_info.devname,
            event.device_info.devpath,
            event.device_info.subsystem,
        );

        if !event.device_info.devtype.is_empty() {
            msg.push_str(&format!("DEVTYPE={}\n", event.device_info.devtype));
        }

        // Add custom properties
        for (key, value) in &event.device_info.properties {
            msg.push_str(&format!("{}={}\n", key, value));
        }

        msg.push('\n'); // Empty line terminates message
        msg
    }

    /// Broadcast a device add event
    pub fn broadcast_add(&self, device_id: DeviceId, config: &DeviceConfig) -> Result<()> {
        let event_node = format!("event{}", device_id);
        let input_node = format!("input{}", device_id);

        let properties = vec![
            ("ID_INPUT".to_string(), "1".to_string()),
            ("ID_INPUT_JOYSTICK".to_string(), "1".to_string()),
            (
                "ID_VENDOR_ID".to_string(),
                format!("{:04x}", config.vendor_id),
            ),
            (
                "ID_MODEL_ID".to_string(),
                format!("{:04x}", config.product_id),
            ),
            (
                "ID_BUS".to_string(),
                match config.bustype {
                    BusType::Usb => "usb".to_string(),
                    BusType::Bluetooth => "bluetooth".to_string(),
                    BusType::Virtual => "virtual".to_string(),
                },
            ),
            ("NAME".to_string(), format!("\"{}\"", config.name)),
            (
                "PRODUCT".to_string(),
                format!(
                    "{:x}/{:x}/{:x}/{:x}",
                    config.bustype as u16, config.vendor_id, config.product_id, config.version
                ),
            ),
        ];

        let event = UdevEvent {
            action: UdevAction::Add,
            device_info: UdevDeviceInfo {
                subsystem: "input".to_string(),
                devtype: "".to_string(),
                devname: format!("/dev/input/{}", event_node),
                devpath: format!("/devices/virtual/input/{}/{}", input_node, event_node),
                syspath: format!("/sys/devices/virtual/input/{}/{}", input_node, event_node),
                properties,
            },
        };

        self.event_tx
            .send(event)
            .map_err(|_| anyhow::anyhow!("No receivers"))?;

        info!("Broadcasted device add event for {}", event_node);

        Ok(())
    }

    /// Broadcast a device remove event
    pub fn broadcast_remove(&self, device_id: DeviceId, config: &DeviceConfig) -> Result<()> {
        let event_node = format!("event{}", device_id);
        let input_node = format!("input{}", device_id);

        let event = UdevEvent {
            action: UdevAction::Remove,
            device_info: UdevDeviceInfo {
                subsystem: "input".to_string(),
                devtype: "".to_string(),
                devname: format!("/dev/input/{}", event_node),
                devpath: format!("/devices/virtual/input/{}/{}", input_node, event_node),
                syspath: format!("/sys/devices/virtual/input/{}/{}", input_node, event_node),
                properties: vec![("NAME".to_string(), format!("\"{}\"", config.name))],
            },
        };

        self.event_tx
            .send(event)
            .map_err(|_| anyhow::anyhow!("No receivers"))?;

        info!("Broadcasted device remove event for {}", event_node);

        Ok(())
    }

    /// Get a clone of the event sender (for other components to broadcast events)
    pub fn event_sender(&self) -> broadcast::Sender<UdevEvent> {
        self.event_tx.clone()
    }
}
