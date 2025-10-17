use crate::manager::VirtualDevice;
use crate::protocol::*;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

/// State of a uinput device being configured
#[derive(Debug, Clone, Default)]
struct UinputDeviceState {
    name: Option<String>,
    vendor_id: u16,
    product_id: u16,
    version: u16,
    bustype: u16,

    // Track what's been enabled
    ev_types: Vec<u16>,
    keys: Vec<u16>,
    abs_axes: HashMap<u16, LinuxAbsEvent>,
    rel_axes: Vec<u16>,

    // Track which session this is
    session_id: Option<ulid::Ulid>,
}
impl UinputDeviceState {
    /// Convert to DeviceConfig for creating a VirtualDevice
    fn to_device_config(&self) -> DeviceConfig {
        let name = self
            .name
            .clone()
            .unwrap_or_else(|| "virtual uinput Device".to_string());

        // Convert keys to buttons
        let buttons = self
            .keys
            .iter()
            .filter_map(|&code| Button::from_ev_code(code))
            .collect();

        // Convert abs axes to axis configs
        let axes = self
            .abs_axes
            .iter()
            .filter_map(|(&code, info)| {
                Axis::from_ev_code(code).map(|axis| AxisConfig {
                    axis,
                    min: info.minimum,
                    max: info.maximum,
                    fuzz: info.fuzz,
                    flat: info.flat,
                })
            })
            .collect();

        DeviceConfig {
            name,
            vendor_id: self.vendor_id,
            product_id: self.product_id,
            version: self.version,
            bustype: match self.bustype {
                0x03 => BusType::Usb,
                0x05 => BusType::Bluetooth,
                _ => BusType::Virtual,
            },
            buttons,
            axes,
        }
    }
}

pub struct UinputEmulator {
    base_path: PathBuf,
    socket_path: PathBuf,
    devices: Arc<Mutex<HashMap<DeviceId, Arc<VirtualDevice>>>>,
    next_device_id: Arc<Mutex<DeviceId>>,
    mirror_map: Arc<Mutex<HashMap<DeviceId, DeviceId>>>,
}
impl UinputEmulator {
    pub fn new(
        base_path: impl AsRef<Path>,
        devices: Arc<Mutex<HashMap<DeviceId, Arc<VirtualDevice>>>>,
        next_device_id: Arc<Mutex<DeviceId>>,
    ) -> Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        let socket_path = base_path.join("uinput");

        Ok(Self {
            base_path,
            socket_path,
            devices,
            next_device_id,
            mirror_map: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn run(&self) -> Result<()> {
        // Remove existing socket if present
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;
        let devices = self.devices.clone();

        // Set socket permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o666))?;
        }

        info!(
            "uinput emulator listening on {}",
            self.socket_path.display()
        );

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let devices = devices.clone();
                    let next_device_id = self.next_device_id.clone();
                    let base_path = self.base_path.clone();
                    let mirror_map = self.mirror_map.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_client(
                            stream,
                            &devices,
                            &next_device_id,
                            &base_path,
                            &mirror_map,
                        )
                        .await
                        {
                            error!("uinput client error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept uinput connection: {}", e);
                }
            }
        }
    }

    pub async fn mirror_to_uinput_devices(
        &self,
        source_device_id: DeviceId,
        events: &Vec<InputEvent>,
    ) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }

        // Get mirror_id without holding lock
        let mirror_id = {
            let map = self.mirror_map.lock().await;
            map.get(&source_device_id).copied()
        };

        if let Some(mirror_id) = mirror_id {
            trace!(
                "Mirroring {} events from device {} to device {}",
                events.len(),
                source_device_id,
                mirror_id
            );

            // Get mirror device
            let mirror_device = {
                let devices = self.devices.lock().await;
                devices.get(&mirror_id).cloned()
            };

            if let Some(mirror_device) = mirror_device {
                match mirror_device.send_events(events).await {
                    Ok(()) => trace!("Mirrored successfully"),
                    Err(e) => warn!("Mirror send failed: {}", e),
                }
            } else {
                trace!("Mirror device {} no longer exists", mirror_id);
            }
        }

        Ok(())
    }

    async fn handle_client(
        mut stream: UnixStream,
        devices: &Arc<Mutex<HashMap<DeviceId, Arc<VirtualDevice>>>>,
        next_device_id: &Arc<Mutex<DeviceId>>,
        base_path: &PathBuf,
        mirror_map: &Arc<Mutex<HashMap<DeviceId, DeviceId>>>,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let session_id = ulid::Ulid::new();
        debug!("New uinput session {}", session_id);

        let mut state = UinputDeviceState::default();
        state.session_id = Some(session_id);
        let mut bound_device_id: Option<DeviceId> = None;
        let mut created_device_id: Option<DeviceId> = None;

        loop {
            // Read 4-byte length prefix
            let mut len_buf = [0u8; 4];
            match stream.read_exact(&mut len_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    debug!("uinput session {} disconnected", session_id);
                    break;
                }
                Err(e) => {
                    error!("Error reading length from session {}: {}", session_id, e);
                    break;
                }
            }

            let msg_len = u32::from_le_bytes(len_buf) as usize;
            if msg_len == 0 || msg_len > 1_000_000 {
                error!(
                    "Invalid message length {} from session {}",
                    msg_len, session_id
                );
                break;
            }

            // Read message body
            let mut msg_buf = vec![0u8; msg_len];
            match stream.read_exact(&mut msg_buf).await {
                Ok(_) => {}
                Err(e) => {
                    error!("Error reading message from session {}: {}", session_id, e);
                    break;
                }
            }

            let request: UinputRequest = match UinputRequest::from_bytes(&msg_buf) {
                Ok(req) => req,
                Err(e) => {
                    error!("Failed to parse request from session {}: {}", session_id, e);
                    continue;
                }
            };

            // Check if this is WriteEvents (fire-and-forget)
            let is_write_events = matches!(request, UinputRequest::WriteEvents { .. });

            trace!("Session {}: request {:?}", session_id, request);

            let response = Self::process_request(
                request,
                &mut state,
                &mut bound_device_id,
                &mut created_device_id,
                devices,
                next_device_id,
                base_path,
                mirror_map,
            )
            .await;

            // For WriteEvents, don't bother sending response (client won't read it anyway)
            if is_write_events {
                trace!(
                    "Session {}: WriteEvents processed (no response sent)",
                    session_id
                );
                continue; // Skip response sending
            }

            // For other requests (setup ioctls), send response normally
            trace!("Session {}: response {:?}", session_id, response);

            let response_bytes = match response.to_bytes() {
                Ok(b) => b,
                Err(e) => {
                    error!("Failed to serialize response: {}", e);
                    continue;
                }
            };

            match stream.write_all(&response_bytes).await {
                Ok(()) => {}
                Err(e) => {
                    error!("Write error session {}: {}", session_id, e);
                    break;
                }
            }

            match stream.flush().await {
                Ok(()) => {}
                Err(e) => {
                    error!("Flush error session {}: {}", session_id, e);
                    break;
                }
            }
        }

        // Cleanup
        if let Some(device_id) = created_device_id {
            info!(
                "Session {} cleanup: removing device {}",
                session_id, device_id
            );
            devices.lock().await.remove(&device_id);
        }

        debug!("uinput session {} exiting", session_id);
        Ok(())
    }

    async fn process_request(
        request: UinputRequest,
        state: &mut UinputDeviceState,
        created_device_id: &mut Option<DeviceId>,
        bound_device_id: &mut Option<DeviceId>,
        devices: &Arc<Mutex<HashMap<DeviceId, Arc<VirtualDevice>>>>,
        next_device_id: &Arc<Mutex<DeviceId>>,
        base_path: &Path,
        mirror_map: &Arc<Mutex<HashMap<DeviceId, DeviceId>>>,
    ) -> UinputResponse {
        match request {
            UinputRequest::SetEvBit { ev_type } => {
                trace!("SetEvBit: {}", ev_type);
                if !state.ev_types.contains(&ev_type) {
                    state.ev_types.push(ev_type);
                }
                UinputResponse {
                    success: true,
                    device_id: None,
                    error: None,
                }
            }

            UinputRequest::SetKeyBit { key_code } => {
                trace!("SetKeyBit: {}", key_code);
                if !state.keys.contains(&key_code) {
                    state.keys.push(key_code);
                }
                UinputResponse {
                    success: true,
                    device_id: None,
                    error: None,
                }
            }

            UinputRequest::SetAbsBit { abs_code } => {
                trace!("SetAbsBit: {}", abs_code);
                // Add with default range if not already configured
                state.abs_axes.entry(abs_code).or_insert(LinuxAbsEvent {
                    value: 0,
                    minimum: -32768,
                    maximum: 32767,
                    fuzz: 16,
                    flat: 128,
                    resolution: 0,
                });
                UinputResponse {
                    success: true,
                    device_id: None,
                    error: None,
                }
            }

            UinputRequest::SetRelBit { rel_code } => {
                trace!("SetRelBit: {}", rel_code);
                if !state.rel_axes.contains(&rel_code) {
                    state.rel_axes.push(rel_code);
                }
                UinputResponse {
                    success: true,
                    device_id: None,
                    error: None,
                }
            }

            UinputRequest::AbsSetup { code, absinfo } => {
                trace!(
                    "AbsSetup: code={}, range=[{}, {}]",
                    code, absinfo.minimum, absinfo.maximum
                );
                state.abs_axes.insert(code, absinfo);
                UinputResponse {
                    success: true,
                    device_id: None,
                    error: None,
                }
            }

            UinputRequest::DevSetup { setup } => {
                trace!("DevSetup: {}", setup.name);
                state.name = Some(setup.name);
                state.vendor_id = setup.vendor_id;
                state.product_id = setup.product_id;
                state.version = setup.version;
                state.bustype = setup.bustype;
                UinputResponse {
                    success: true,
                    device_id: None,
                    error: None,
                }
            }

            UinputRequest::DevCreate {} => {
                let config = state.to_device_config();
                info!(
                    "DevCreate session {:?}: Creating mirror device for Steam Input",
                    state.session_id
                );

                // Find the FIRST device (the one external app is controlling)
                let source_device_id = {
                    let devices_lock = devices.lock().await;
                    devices_lock.keys().min().copied()
                };

                if source_device_id.is_none() {
                    warn!("No source device to mirror!");
                    return UinputResponse {
                        success: false,
                        device_id: None,
                        error: Some("No source device".to_string()),
                    };
                }
                let source_device_id = source_device_id.unwrap();

                // Create NEW device for Steam's output
                let mirror_device_id = {
                    let mut next_id = next_device_id.lock().await;
                    let id = *next_id;
                    *next_id += 1;
                    id
                };

                match VirtualDevice::create(mirror_device_id, config.clone(), base_path).await {
                    Ok(device) => {
                        let event_node = device.event_node.clone();
                        devices
                            .lock()
                            .await
                            .insert(mirror_device_id, Arc::new(device));

                        // Set up mirroring: source_device -> mirror_device
                        mirror_map
                            .lock()
                            .await
                            .insert(source_device_id, mirror_device_id);

                        info!(
                            "Session {:?}: Created mirror device {} as {} (mirrors device {})",
                            state.session_id, mirror_device_id, event_node, source_device_id
                        );

                        *bound_device_id = Some(mirror_device_id);
                        *created_device_id = Some(mirror_device_id);

                        UinputResponse {
                            success: true,
                            device_id: Some(mirror_device_id),
                            error: None,
                        }
                    }
                    Err(e) => {
                        error!("Failed to create mirror device: {}", e);
                        UinputResponse {
                            success: false,
                            device_id: None,
                            error: Some(format!("Create failed: {}", e)),
                        }
                    }
                }
            }

            UinputRequest::DevDestroy {} => {
                if let Some(device_id) = created_device_id.take() {
                    info!(
                        "Session {:?}: Destroying mirror device {}",
                        state.session_id, device_id
                    );

                    // Remove from devices first
                    devices.lock().await.remove(&device_id);

                    // Remove mirror mapping
                    {
                        let mut map = mirror_map.lock().await;
                        let to_remove: Vec<_> = map
                            .iter()
                            .filter(|&(_, &mirror)| mirror == device_id)
                            .map(|(&source, _)| source)
                            .collect();

                        for source_id in to_remove {
                            map.remove(&source_id);
                            info!("Removed mirror mapping {} -> {}", source_id, device_id);
                        }
                    }
                }
                *bound_device_id = None;

                UinputResponse {
                    success: true,
                    device_id: None,
                    error: None,
                }
            }

            UinputRequest::WriteEvents { events } => {
                trace!(
                    "WriteEvents: session {:?}, {} events",
                    state.session_id,
                    events.len()
                );
                if events.is_empty() || bound_device_id.is_none() {
                    return UinputResponse {
                        success: true,
                        device_id: *bound_device_id,
                        error: None,
                    };
                }

                let device_id = bound_device_id.unwrap();

                trace!(
                    "Session {:?}: Forwarding {} remapped events to device {}",
                    state.session_id,
                    events.len(),
                    device_id
                );

                // Convert to InputEvents and forward to the mirror device
                let input_events: Vec<InputEvent> = events
                    .iter()
                    .filter_map(|e| match e.event_type {
                        EV_KEY => Button::from_ev_code(e.code).map(|button| InputEvent::Button {
                            button,
                            pressed: e.value != 0,
                        }),
                        EV_ABS => Axis::from_ev_code(e.code).map(|axis| InputEvent::Axis {
                            axis,
                            value: e.value,
                        }),
                        EV_SYN => Some(InputEvent::Sync),
                        _ => None,
                    })
                    .collect();

                if input_events.is_empty() {
                    return UinputResponse {
                        success: true,
                        device_id: Some(device_id),
                        error: None,
                    };
                }

                // Forward to mirror device (device1)
                let device = {
                    let devices_lock = devices.lock().await;
                    devices_lock.get(&device_id).cloned()
                };

                if let Some(device) = device {
                    match device.send_events(&input_events).await {
                        Ok(()) => {
                            trace!("Forwarded successfully to device {}", device_id);
                            UinputResponse {
                                success: true,
                                device_id: Some(device_id),
                                error: None,
                            }
                        }
                        Err(e) => {
                            error!("Failed to forward to device {}: {}", device_id, e);
                            UinputResponse {
                                success: false,
                                device_id: Some(device_id),
                                error: Some(format!("Forward error: {}", e)),
                            }
                        }
                    }
                } else {
                    error!("Device {} no longer exists", device_id);
                    UinputResponse {
                        success: false,
                        device_id: None,
                        error: Some("Device gone".to_string()),
                    }
                }
            }
        }
    }
}
