use crate::protocol::*;
use anyhow::Result;
use std::mem::size_of;
use std::path::Path;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
use tracing::{debug, error, info};

#[repr(C)]
struct MonitorNetlinkHeader {
    prefix: [u8; 8], // "libudev\0"
    magic: u32,      // 0xfeedcafe (big-endian)
    header_size: u32,
    properties_off: u32,
    properties_len: u32,
    filter_subsystem_hash: u32,
    filter_devtype_hash: u32,
    filter_tag_bloom_hi: u32,
    filter_tag_bloom_lo: u32,
}

/// MurmurHash2 - needed for subsystem/devtype hashing
fn murmur_hash2(data: &[u8], seed: u32) -> u32 {
    const M: u32 = 0x5bd1e995;
    const R: i32 = 24;

    let mut h: u32 = seed ^ (data.len() as u32);
    let mut chunks = data.chunks_exact(4);

    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M);
        h ^= k;
    }

    let remainder = chunks.remainder();
    match remainder.len() {
        3 => {
            h ^= (remainder[2] as u32) << 16;
            h ^= (remainder[1] as u32) << 8;
            h ^= remainder[0] as u32;
            h = h.wrapping_mul(M);
        }
        2 => {
            h ^= (remainder[1] as u32) << 8;
            h ^= remainder[0] as u32;
            h = h.wrapping_mul(M);
        }
        1 => {
            h ^= remainder[0] as u32;
            h = h.wrapping_mul(M);
        }
        _ => {}
    }

    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    h
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

/// Udev event broadcaster
pub struct UdevBroadcaster {
    listener: UnixListener,
    event_tx: broadcast::Sender<UdevEvent>,
}
impl UdevBroadcaster {
    /// Create a new udev broadcaster
    pub fn new(base_path: &Path) -> Result<Self> {
        let socket_path = base_path.join("udev");

        // Remove old socket if exists
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path)?;

        info!("udev socket created at {}", socket_path.display());

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
                Ok((stream, _addr)) => {
                    info!("udev monitor connected");

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
        stream: UnixStream,
        event_rx: &mut broadcast::Receiver<UdevEvent>,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Split stream into read and write halves
        let (mut read_half, mut write_half) = stream.into_split();

        // Spawn task to READ from monitor (discard filter commands)
        tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            loop {
                match read_half.read(&mut buf).await {
                    Ok(0) => {
                        break;
                    }
                    Ok(_n) => {
                        // Just discard - libudev sending filter updates
                    }
                    Err(e) => {
                        debug!("Monitor read error: {}", e);
                        break;
                    }
                }
            }
        });

        // WRITE events to monitor
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    let message = Self::format_udev_message(&event);
                    write_half.write_all(&message).await?;
                    write_half.flush().await?;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!("Monitor lagged {} events", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(anyhow::anyhow!("Event channel closed"));
                }
            }
        }
    }

    /// Format a udev event
    pub(crate) fn format_udev_message(event: &UdevEvent) -> Vec<u8> {
        let action = match event.action {
            UdevAction::Add => "add",
            UdevAction::Remove => "remove",
            UdevAction::Change => "change",
        };

        // Build properties string (null-separated, double-null terminated)
        let mut properties = String::new();
        properties.push_str(&format!("ACTION={}\0", action));
        properties.push_str(&format!("DEVPATH={}\0", event.device_info.devpath));
        properties.push_str(&format!("SUBSYSTEM={}\0", event.device_info.subsystem));
        properties.push_str(&format!("DEVNAME={}\0", event.device_info.devname));

        for (key, value) in &event.device_info.properties {
            properties.push_str(&format!("{}={}\0", key, value));
        }
        properties.push('\0'); // Double null terminator

        // Calculate hashes for filtering
        let subsystem_hash = murmur_hash2(event.device_info.subsystem.as_bytes(), 0);
        let devtype_hash = if !event.device_info.devtype.is_empty() {
            murmur_hash2(event.device_info.devtype.as_bytes(), 0)
        } else {
            0
        };

        // Build header
        let header = MonitorNetlinkHeader {
            prefix: *b"libudev\0",
            magic: 0xfeedcafe_u32.to_be(),
            header_size: size_of::<MonitorNetlinkHeader>() as u32,
            properties_off: size_of::<MonitorNetlinkHeader>() as u32,
            properties_len: properties.len() as u32,
            filter_subsystem_hash: subsystem_hash.to_be(),
            filter_devtype_hash: devtype_hash.to_be(),
            filter_tag_bloom_hi: 0,
            filter_tag_bloom_lo: 0,
        };

        // Combine header + properties
        let mut message = Vec::new();

        // Copy header as bytes
        unsafe {
            let header_bytes = std::slice::from_raw_parts(
                &header as *const _ as *const u8,
                size_of::<MonitorNetlinkHeader>(),
            );
            message.extend_from_slice(header_bytes);
        }

        // Add properties
        message.extend_from_slice(properties.as_bytes());

        message
    }

    /// Broadcast a device add event
    pub fn broadcast_add(&self, device_id: DeviceId, config: &DeviceConfig) -> Result<()> {
        let event_node = format!("event{}", device_id);
        let input_node = format!("input{}", device_id);

        let unique_name = format!("{} ({})", config.name, event_node);

        let mut properties = vec![
            ("ID_INPUT".to_string(), "1".to_string()),
            ("ID_INPUT_JOYSTICK".to_string(), "1".to_string()),
            (
                "ID_MODEL".to_string(),
                format!("{}_{}", config.name.replace(' ', "_"), device_id),
            ),
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
            ("NAME".to_string(), format!("\"{}\"", unique_name)),
            (
                "PRODUCT".to_string(),
                format!(
                    "{:x}/{:x}/{:x}/{:x}",
                    config.bustype as u16, config.vendor_id, config.product_id, config.version
                ),
            ),
            ("ID_SERIAL".to_string(), format!("vimputti_{}", event_node)),
            ("ID_SERIAL_SHORT".to_string(), event_node.clone()),
            ("UNIQ".to_string(), event_node.clone()),
        ];

        if matches!(config.bustype, BusType::Usb) {
            properties.push(("BUSNUM".to_string(), "253".to_string()));
            properties.push(("DEVNUM".to_string(), format!("{:03}", device_id + 1)));
        }

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

        let unique_name = format!("{} ({})", config.name, event_node);

        let mut event = UdevEvent {
            action: UdevAction::Remove,
            device_info: UdevDeviceInfo {
                subsystem: "input".to_string(),
                devtype: "".to_string(),
                devname: format!("/dev/input/{}", event_node),
                devpath: format!("/devices/virtual/input/{}/{}", input_node, event_node),
                syspath: format!("/sys/devices/virtual/input/{}/{}", input_node, event_node),
                properties: vec![
                    ("NAME".to_string(), format!("\"{}\"", unique_name)),
                    (
                        "ID_MODEL".to_string(),
                        format!("{}_{}", config.name.replace(' ', "_"), device_id),
                    ),
                    ("ID_SERIAL".to_string(), format!("vimputti_{}", event_node)),
                    ("ID_SERIAL_SHORT".to_string(), event_node.clone()),
                    ("UNIQ".to_string(), event_node.clone()),
                ],
            },
        };

        if matches!(config.bustype, BusType::Usb) {
            event
                .device_info
                .properties
                .push(("BUSNUM".to_string(), "253".to_string()));
            event
                .device_info
                .properties
                .push(("DEVNUM".to_string(), format!("{:03}", device_id + 1)));
        }

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
