use crate::client::ClientInner;
use crate::client::batch::BatchManager;
use crate::protocol::*;
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tracing::debug;

/// Handle to a virtual input device
///
/// This struct provides a high-level API for sending input events to a virtual device.
/// Events are automatically batched and flushed after a configurable timeout
/// or when explicitly flushed.
///
/// The device is automatically destroyed when this handle is dropped.
pub struct VirtualController {
    client: Arc<ClientInner>,
    device_id: DeviceId,
    event_node: String,
    batch_manager: BatchManager,
}
impl VirtualController {
    pub(crate) fn new(client: Arc<ClientInner>, device_id: DeviceId, event_node: String) -> Self {
        let batch_manager =
            BatchManager::new(Arc::clone(&client), device_id, Duration::from_millis(1));

        Self {
            client,
            device_id,
            event_node,
            batch_manager,
        }
    }

    /// Get the device ID
    pub fn device_id(&self) -> DeviceId {
        self.device_id
    }

    /// Get the event node name (e.g., "event0")
    pub fn event_node(&self) -> &str {
        &self.event_node
    }

    /// Set the auto-flush timeout for batched events
    pub fn set_batch_timeout(&mut self, timeout: Duration) {
        self.batch_manager.set_timeout(timeout);
    }

    /// Press or release a button
    pub fn button(&self, button: Button, pressed: bool) {
        self.batch_manager
            .queue_event(InputEvent::Button { button, pressed });
    }

    /// Convenience method to press a button
    pub fn button_press(&self, button: Button) {
        self.button(button, true);
    }

    /// Convenience method to release a button
    pub fn button_release(&self, button: Button) {
        self.button(button, false);
    }

    /// Move an axis to a specific value
    pub fn axis(&self, axis: Axis, value: i32) {
        self.batch_manager
            .queue_event(InputEvent::Axis { axis, value });
    }

    /// Send a raw Linux input event
    pub fn raw_event(&self, event_type: u16, code: u16, value: i32) {
        self.batch_manager.queue_event(InputEvent::Raw {
            event_type,
            code,
            value,
        });
    }

    /// Sends a sync (SYN_REPORT) event
    pub fn sync(&self) {
        self.batch_manager.queue_event(InputEvent::Sync);
    }

    /// Manually flush all pending events immediately
    ///
    /// Events are automatically flushed after the batch timeout,
    /// however this method allows immediate flushing for specific needs.
    pub async fn flush(&self) -> Result<()> {
        self.batch_manager.flush().await
    }

    /// Send events and wait for them to be delivered
    ///
    /// This is useful when you want to ensure events are sent immediately
    /// without relying on auto-batching.
    pub async fn send_events(&self, events: Vec<InputEvent>) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let id = ulid::Ulid::new().to_string();
        let command = ControlCommand::SendInput {
            device_id: self.device_id,
            events,
        };
        let message = ControlMessage {
            id: id.clone(),
            command,
        };

        let message_json = serde_json::to_string(&message)?;

        let mut stream = self.client.stream.lock().await;

        // Send command
        stream.write_all(message_json.as_bytes()).await?;
        stream.write_all(b"\n").await?;

        // Read response
        let mut reader = BufReader::new(&mut *stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await?;

        let response: ControlResponse = serde_json::from_str(&response_line)?;

        if response.id != id {
            anyhow::bail!("Response ID mismatch");
        }

        match response.result {
            ControlResult::InputSent => Ok(()),
            ControlResult::Error { message } => {
                anyhow::bail!("Failed to send input: {}", message)
            }
            _ => anyhow::bail!("Unexpected response to SendInput"),
        }
    }
}
impl Drop for VirtualController {
    fn drop(&mut self) {
        let client = Arc::clone(&self.client);
        let device_id = self.device_id;

        // Spawn cleanup task
        tokio::spawn(async move {
            let id = ulid::Ulid::new().to_string();
            let command = ControlCommand::DestroyDevice { device_id };
            let message = ControlMessage {
                id: id.clone(),
                command,
            };

            if let Ok(message_json) = serde_json::to_string(&message) {
                let mut stream = client.stream.lock().await;
                let _ = stream.write_all(message_json.as_bytes()).await;
                let _ = stream.write_all(b"\n").await;

                // Read response (but don't wait too long)
                let mut reader = BufReader::new(&mut *stream);
                let mut response_line = String::new();
                let _ = reader.read_line(&mut response_line).await;
            }

            debug!("Device {} destroyed", device_id);
        });
    }
}
