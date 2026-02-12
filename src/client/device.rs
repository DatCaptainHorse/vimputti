use crate::client::ClientInner;
use crate::protocol::*;
use anyhow::Result;
use std::sync::Arc;

/// Handle to a virtual input device
///
/// This struct provides a high-level API for sending input events to a virtual device.
/// The device is automatically destroyed when this handle is dropped.
pub struct VirtualController {
    client: Arc<ClientInner>,
    device_id: DeviceId,
    event_node: String,
}

impl VirtualController {
    pub(crate) fn new(client: Arc<ClientInner>, device_id: DeviceId, event_node: String) -> Self {
        Self {
            client,
            device_id,
            event_node,
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

    /// Press or release a button
    pub async fn button(&self, button: Button, pressed: bool) -> Result<()> {
        self.send_events(vec![InputEvent::Button { button, pressed }])
            .await
    }

    /// Convenience method to press a button
    pub async fn button_press(&self, button: Button) -> Result<()> {
        self.button(button, true).await
    }

    /// Convenience method to release a button
    pub async fn button_release(&self, button: Button) -> Result<()> {
        self.button(button, false).await
    }

    /// Move an axis to a specific value
    pub async fn axis(&self, axis: Axis, value: i32) -> Result<()> {
        self.send_events(vec![InputEvent::Axis { axis, value }])
            .await
    }

    /// Send a raw Linux input event
    pub async fn raw_event(&self, event_type: u16, code: u16, value: i32) -> Result<()> {
        self.send_events(vec![InputEvent::Raw {
            event_type,
            code,
            value,
        }])
        .await
    }

    /// Sends a sync (SYN_REPORT) event
    pub async fn sync(&self) -> Result<()> {
        self.send_events(vec![InputEvent::Sync]).await
    }

    /// Poll for force feedback events (e.g., rumble)
    pub async fn poll_feedback(&self) -> Result<Option<FeedbackEvent>> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let id = ulid::Ulid::new().to_string();
        let command = ControlCommand::PollFeedback {
            device_id: self.device_id,
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
            ControlResult::FeedbackPolled { event } => Ok(event),
            ControlResult::Error { message } => {
                anyhow::bail!("Failed to poll feedback: {}", message)
            }
            _ => anyhow::bail!("Unexpected response to PollFeedback"),
        }
    }

    /// Send events and wait for them to be delivered
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
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

            tracing::debug!("Device {} destroyed", device_id);
        });
    }
}
