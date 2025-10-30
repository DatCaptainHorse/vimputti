use crate::client::ClientInner;
use crate::protocol::*;
use anyhow::Result;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::broadcast;
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
    feedback_rx: Option<broadcast::Receiver<FeedbackEvent>>,
}
impl VirtualController {
    pub(crate) fn new(client: Arc<ClientInner>, device_id: DeviceId, event_node: String) -> Self {
        Self {
            client,
            device_id,
            event_node,
            feedback_rx: None,
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

    /// Enable force feedback support
    async fn enable_feedback(&mut self) -> Result<()> {
        let base_path = self.client.get_base_path();
        let feedback_path = format!("{}/devices/{}.feedback", base_path, self.event_node);

        tracing::info!("Connecting to feedback socket: {}", feedback_path);
        let stream = UnixStream::connect(&feedback_path).await?;
        tracing::info!("Connected to feedback socket!");

        let (tx, rx) = broadcast::channel(100);

        tokio::spawn(async move {
            let mut buf = vec![0u8; 24];
            let mut stream = stream;

            // State to collect rumble info
            let mut pending_strong = 0u16;
            let mut pending_weak = 0u16;
            let mut pending_duration = 0u16;

            loop {
                match stream.read_exact(&mut buf).await {
                    Ok(_) => {
                        let event: LinuxInputEvent =
                            unsafe { std::ptr::read(buf.as_ptr() as *const _) };

                        debug!(
                            "FF event: type={}, code={}, value={}",
                            event.event_type, event.code, event.value
                        );

                        if event.event_type == EV_FF {
                            if event.code == FF_RUMBLE {
                                if event.value == 0 {
                                    // Stop rumble
                                    let feedback = FeedbackEvent::RumbleStop;
                                    debug!("Sending rumble stop");
                                    let _ = tx.send(feedback);
                                } else {
                                    // Parse magnitudes
                                    pending_strong = (event.value >> 16) as u16;
                                    pending_weak = (event.value & 0xFFFF) as u16;
                                }
                            } else if event.code == FF_RUMBLE + 1 {
                                // Parse duration
                                pending_duration = event.value as u16;

                                // Now we have all info, send the complete event
                                let feedback = FeedbackEvent::Rumble {
                                    strong_magnitude: pending_strong,
                                    weak_magnitude: pending_weak,
                                    duration_ms: pending_duration,
                                };

                                debug!(
                                    "Sending rumble: strong={}, weak={}, duration={}ms",
                                    pending_strong, pending_weak, pending_duration
                                );
                                let _ = tx.send(feedback);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error reading from feedback socket: {}", e);
                        break;
                    }
                }
            }
        });

        self.feedback_rx = Some(rx);
        Ok(())
    }

    /// Register a callback for rumble events
    pub async fn on_rumble<F>(&mut self, mut callback: F) -> Result<tokio::task::JoinHandle<()>>
    where
        F: FnMut(u16, u16, u16) + Send + 'static, // (strong, weak, duration_ms)
    {
        if self.feedback_rx.is_none() {
            self.enable_feedback().await?;
        }

        let mut rx = self.feedback_rx.as_ref().unwrap().resubscribe();

        let handle = tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    FeedbackEvent::Rumble {
                        strong_magnitude,
                        weak_magnitude,
                        duration_ms,
                    } => {
                        callback(strong_magnitude, weak_magnitude, duration_ms);
                    }
                    FeedbackEvent::RumbleStop => {
                        callback(0, 0, 0); // Stop = zero magnitudes
                    }
                    _ => {}
                }
            }
        });

        Ok(handle)
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
