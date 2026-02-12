use crate::protocol::*;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::debug;

mod device;

pub use device::VirtualController;

pub(crate) struct ClientInner {
    stream: Mutex<UnixStream>,
    socket_path: String,
}
impl ClientInner {
    pub(crate) fn get_base_path(&self) -> String {
        // Prefer env var if set
        if let Ok(base) = std::env::var("VIMPUTTI_BASE_PATH") {
            return base;
        }

        // Manager creates base_path as socket_path.parent()/vimputti
        // So for socket /tmp/vimputti-0, base is /tmp/vimputti
        let socket_path = Path::new(&self.socket_path);
        socket_path
            .parent()
            .unwrap_or_else(|| Path::new("/tmp"))
            .join("vimputti")
            .to_string_lossy()
            .to_string()
    }
}

/// Client for communicating with the vimputti manager
pub struct VimputtiClient {
    inner: Arc<ClientInner>,
}
impl VimputtiClient {
    /// Connect to a vimputti manager instance
    pub async fn connect(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_string_lossy().to_string();

        let stream = UnixStream::connect(&socket_path)
            .await
            .with_context(|| format!("Failed to connect to manager at {}", socket_path))?;

        debug!("Connected to vimputti manager at {}", socket_path);

        Ok(Self {
            inner: Arc::new(ClientInner {
                stream: Mutex::new(stream),
                socket_path,
            }),
        })
    }

    /// Connect to default vimputti manager (instance 0)
    pub async fn connect_default() -> Result<Self> {
        Self::connect("/tmp/vimputti-0").await
    }

    /// Ping the manager to check if it's alive
    pub async fn ping(&self) -> Result<()> {
        let response = self.send_command(ControlCommand::Ping).await?;
        match response {
            ControlResult::Pong => Ok(()),
            ControlResult::Error { message } => {
                anyhow::bail!("Manager returned error: {}", message)
            }
            _ => anyhow::bail!("Unexpected response to ping"),
        }
    }

    /// Create a new virtual device from a configuration
    pub async fn create_device(&self, config: DeviceConfig) -> Result<VirtualController> {
        let response = self
            .send_command(ControlCommand::CreateDevice { config })
            .await?;

        match response {
            ControlResult::DeviceCreated {
                device_id,
                event_node,
            } => {
                debug!("Created device {} as {}", device_id, event_node);
                Ok(VirtualController::new(
                    Arc::clone(&self.inner),
                    device_id,
                    event_node,
                ))
            }
            ControlResult::Error { message } => {
                anyhow::bail!("Failed to create device: {}", message)
            }
            _ => anyhow::bail!("Unexpected response to CreateDevice"),
        }
    }

    /// List all active devices
    pub async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let response = self.send_command(ControlCommand::ListDevices).await?;

        match response {
            ControlResult::DeviceList(devices) => Ok(devices),
            ControlResult::Error { message } => {
                anyhow::bail!("Failed to list devices: {}", message)
            }
            _ => anyhow::bail!("Unexpected response to ListDevices"),
        }
    }

    /// Send a command to the manager and wait for response
    pub(crate) async fn send_command(&self, command: ControlCommand) -> Result<ControlResult> {
        let id = ulid::Ulid::new().to_string();
        let message = ControlMessage {
            id: id.clone(),
            command,
        };

        let message_json = serde_json::to_string(&message)?;

        let mut stream = self.inner.stream.lock().await;

        // Send command
        stream.write_all(message_json.as_bytes()).await?;
        stream.write_all(b"\n").await?;

        // Read response
        let mut reader = BufReader::new(&mut *stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await?;

        let response: ControlResponse = serde_json::from_str(&response_line)
            .with_context(|| format!("Failed to parse response: {}", response_line))?;

        if response.id != id {
            anyhow::bail!("Response ID mismatch: expected {}, got {}", id, response.id);
        }

        Ok(response.result)
    }
}
impl Clone for VimputtiClient {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
