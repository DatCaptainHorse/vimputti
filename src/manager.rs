use crate::protocol::{DeviceCommand, DeviceResponse, Message, Response};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

// Represents a virtual input device
#[derive(Debug, Clone)]
pub struct VirtualDevice {
    pub name: String,
    pub phys: String,
    pub uniq: String,
    pub id_bustype: u16,
    pub id_vendor: u16,
    pub id_product: u16,
    pub id_version: u16,
    pub driver_version: u32,
    pub enabled_event_types: HashMap<u32, Vec<u32>>,
}

// Represents a virtual uinput device
#[derive(Debug, Clone)]
pub struct VirtualUinputDevice {
    pub device_ptr: u64,
    pub device: VirtualDevice,
}

// Manager for virtual input devices
pub struct InputManager {
    devices: Arc<Mutex<HashMap<u64, VirtualDevice>>>,
    uinput_devices: Arc<Mutex<HashMap<u64, VirtualUinputDevice>>>,
    socket_path: String,
}

impl InputManager {
    pub fn new(socket_path: String) -> Self {
        Self {
            devices: Arc::new(Mutex::new(HashMap::new())),
            uinput_devices: Arc::new(Mutex::new(HashMap::new())),
            socket_path,
        }
    }

    pub async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Remove the socket file if it exists
        if Path::new(&self.socket_path).exists() {
            fs::remove_file(&self.socket_path)?;
        }

        // Create the socket directory if it doesn't exist
        if let Some(parent) = Path::new(&self.socket_path).parent() {
            fs::create_dir_all(parent)?;
        }

        // Bind to the socket
        let listener = UnixListener::bind(&self.socket_path)?;

        // Set socket permissions
        fs::set_permissions(&self.socket_path, fs::Permissions::from_mode(0o777))?;

        tracing::info!("Vimputti manager listening on {}", self.socket_path);

        // Handle incoming connections
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let devices = Arc::clone(&self.devices);
                    let uinput_devices = Arc::clone(&self.uinput_devices);

                    // Spawn a single task to handle this connection
                    tokio::spawn(async move {
                        Self::handle_connection(stream, devices, uinput_devices).await;
                    });
                }
                Err(e) => {
                    tracing::error!("Error accepting connection: {}", e);
                }
            }
        }
    }

    async fn handle_connection(
        mut stream: UnixStream,
        devices: Arc<Mutex<HashMap<u64, VirtualDevice>>>,
        uinput_devices: Arc<Mutex<HashMap<u64, VirtualUinputDevice>>>,
    ) {
        let mut buffer = [0; 4096];
        let mut data = Vec::new();

        loop {
            tokio::select! {
                // Handle incoming data from the socket
                result = stream.read(&mut buffer) => {
                    match result {
                        Ok(0) => break, // Connection closed
                        Ok(n) => {
                            data.extend_from_slice(&buffer[..n]);

                            // Process complete messages
                            while let Some(pos) = data.iter().position(|&b| b == b'\n') {
                                let message_data = data.drain(..=pos).collect::<Vec<_>>();
                                let message_str = String::from_utf8_lossy(&message_data);

                                tracing::info!("Received message: {}", message_str);

                                if let Ok(message) = serde_json::from_str::<Message>(&message_str) {
                                    // Process the message
                                    let response = Self::process_message(message, &devices, &uinput_devices).await;

                                    // Send the response back
                                    if let Ok(response_json) = serde_json::to_string(&response) {
                                        let _ = stream.write_all(response_json.as_bytes()).await;
                                        let _ = stream.write_u8(b'\n').await;
                                        tracing::info!("Sent response: {}", response_json);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Error reading from socket: {}", e);
                            break;
                        }
                    }
                }
                // Handle any other tasks
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    // Just a placeholder to keep the select! running
                }
            }
        }
    }

    async fn process_message(
        message: Message,
        devices: &Arc<Mutex<HashMap<u64, VirtualDevice>>>,
        uinput_devices: &Arc<Mutex<HashMap<u64, VirtualUinputDevice>>>,
    ) -> Response {
        let mut devices = devices.lock().await;
        let mut uinput_devices = uinput_devices.lock().await;
        let response = match message.command {
            DeviceCommand::New { ptr } => {
                let device = VirtualDevice {
                    name: String::new(),
                    phys: String::new(),
                    uniq: String::new(),
                    id_bustype: 0,
                    id_vendor: 0,
                    id_product: 0,
                    id_version: 0,
                    driver_version: 0,
                    enabled_event_types: HashMap::new(),
                };

                devices.insert(ptr, device);
                DeviceResponse::Success
            }
            DeviceCommand::SetName { ptr, name } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device.name = name;
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::SetPhys { ptr, phys } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device.phys = phys;
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::SetUniq { ptr, uniq } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device.uniq = uniq;
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::SetIdBustype { ptr, bustype } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device.id_bustype = bustype;
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::SetIdVendor { ptr, vendor } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device.id_vendor = vendor;
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::SetIdProduct { ptr, product } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device.id_product = product;
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::SetIdVersion { ptr, version } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device.id_version = version;
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::SetDriverVersion { ptr, version } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device.driver_version = version;
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::EnableEventType { ptr, type_ } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device
                        .enabled_event_types
                        .entry(type_)
                        .or_insert_with(Vec::new);
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::EnableEventCode { ptr, type_, code } => {
                if let Some(device) = devices.get_mut(&ptr) {
                    device
                        .enabled_event_types
                        .entry(type_)
                        .or_insert_with(Vec::new)
                        .push(code);
                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::UinputCreateFromDevice { ptr, uinput_ptr } => {
                if let Some(device) = devices.get(&ptr) {
                    let uinput_device = VirtualUinputDevice {
                        device_ptr: ptr,
                        device: device.clone(),
                    };

                    uinput_devices.insert(uinput_ptr, uinput_device);
                    DeviceResponse::UinputCreated { uinput_ptr }
                } else {
                    DeviceResponse::Error {
                        message: format!("Device {} not found", ptr),
                    }
                }
            }
            DeviceCommand::Free { ptr } => {
                devices.remove(&ptr);
                DeviceResponse::Success
            }
            DeviceCommand::UinputDestroy { uinput_ptr } => {
                uinput_devices.remove(&uinput_ptr);
                DeviceResponse::Success
            }
            DeviceCommand::UinputWriteEvent {
                uinput_ptr,
                type_,
                code,
                value,
            } => {
                if let Some(uinput_device) = uinput_devices.get(&uinput_ptr) {
                    // Process the input event
                    tracing::info!(
                        "Input event: type={}, code={}, value={}, device={}",
                        type_,
                        code,
                        value,
                        uinput_device.device.name
                    );

                    // Here you would implement the actual input emulation
                    // For now, we just log the event

                    DeviceResponse::Success
                } else {
                    DeviceResponse::Error {
                        message: format!("Uinput device {} not found", uinput_ptr),
                    }
                }
            }
        };

        Response {
            id: message.id,
            response,
        }
    }
}
