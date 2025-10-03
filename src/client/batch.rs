use crate::client::ClientInner;
use crate::protocol::*;
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};
use tracing::debug;

/// Manages automatic batching and flushing of input events
pub struct BatchManager {
    device_id: DeviceId,
    pending_events: Arc<Mutex<Vec<InputEvent>>>,
    timeout: Arc<Mutex<Duration>>,
    last_event_time: Arc<Mutex<Option<Instant>>>,
    flush_tx: tokio::sync::mpsc::UnboundedSender<FlushRequest>,
}

struct FlushRequest {
    events: Vec<InputEvent>,
    response_tx: tokio::sync::oneshot::Sender<Result<()>>,
}

impl BatchManager {
    pub fn new(client: Arc<ClientInner>, device_id: DeviceId, timeout: Duration) -> Self {
        let pending_events = Arc::new(Mutex::new(Vec::new()));
        let timeout_arc = Arc::new(Mutex::new(timeout));
        let last_event_time = Arc::new(Mutex::new(None));

        // Create flush channel
        let (flush_tx, mut flush_rx) = tokio::sync::mpsc::unbounded_channel::<FlushRequest>();

        // Spawn flush handler task
        let client_clone = Arc::clone(&client);
        tokio::spawn(async move {
            while let Some(request) = flush_rx.recv().await {
                let result =
                    Self::send_events_internal(&client_clone, device_id, request.events).await;
                let _ = request.response_tx.send(result);
            }
        });

        // Spawn auto-flush task
        let pending_clone = Arc::clone(&pending_events);
        let timeout_clone = Arc::clone(&timeout_arc);
        let last_time_clone = Arc::clone(&last_event_time);
        let flush_tx_clone = flush_tx.clone();

        tokio::spawn(async move {
            Self::auto_flush_loop(
                device_id,
                pending_clone,
                timeout_clone,
                last_time_clone,
                flush_tx_clone,
            )
            .await;
        });

        Self {
            device_id,
            pending_events,
            timeout: timeout_arc,
            last_event_time,
            flush_tx,
        }
    }

    /// Set the auto-flush timeout
    pub fn set_timeout(&self, timeout: Duration) {
        let timeout_arc = Arc::clone(&self.timeout);
        tokio::spawn(async move {
            *timeout_arc.lock().await = timeout;
        });
    }

    /// Queue an event for batching
    pub fn queue_event(&self, event: InputEvent) {
        let pending = Arc::clone(&self.pending_events);
        let last_time = Arc::clone(&self.last_event_time);

        tokio::spawn(async move {
            pending.lock().await.push(event);
            *last_time.lock().await = Some(Instant::now());
        });
    }

    /// Manually flush all pending events
    pub async fn flush(&self) -> Result<()> {
        let mut events = self.pending_events.lock().await;
        if events.is_empty() {
            return Ok(());
        }

        let events_to_send = events.drain(..).collect::<Vec<_>>();
        drop(events); // Release lock before sending

        // Send flush request and wait for response
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.flush_tx
            .send(FlushRequest {
                events: events_to_send,
                response_tx,
            })
            .map_err(|_| anyhow::anyhow!("Flush channel closed"))?;

        // Wait for response
        response_rx
            .await
            .map_err(|_| anyhow::anyhow!("Flush response channel closed"))??;

        // Reset last event time
        *self.last_event_time.lock().await = None;

        Ok(())
    }

    /// Auto-flush loop that runs in the background
    async fn auto_flush_loop(
        _device_id: DeviceId,
        pending_events: Arc<Mutex<Vec<InputEvent>>>,
        timeout: Arc<Mutex<Duration>>,
        last_event_time: Arc<Mutex<Option<Instant>>>,
        flush_tx: tokio::sync::mpsc::UnboundedSender<FlushRequest>,
    ) {
        loop {
            sleep(Duration::from_micros(10)).await; // Check every 10µs

            let should_flush = {
                let last_time = last_event_time.lock().await;
                if let Some(last) = *last_time {
                    let timeout_val = *timeout.lock().await;
                    last.elapsed() >= timeout_val
                } else {
                    false
                }
            };

            if should_flush {
                let mut events = pending_events.lock().await;
                if !events.is_empty() {
                    let events_to_send = events.drain(..).collect::<Vec<_>>();
                    drop(events); // Release lock before sending

                    // Send flush request (don't wait for response in auto-flush)
                    let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
                    if flush_tx
                        .send(FlushRequest {
                            events: events_to_send,
                            response_tx,
                        })
                        .is_err()
                    {
                        debug!("Flush channel closed, stopping auto-flush loop");
                        break;
                    }

                    *last_event_time.lock().await = None;
                }
            }
        }
    }

    /// Internal method to send events to the manager
    async fn send_events_internal(
        client: &Arc<ClientInner>,
        device_id: DeviceId,
        events: Vec<InputEvent>,
    ) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let id = ulid::Ulid::new().to_string();
        let command = ControlCommand::SendInput { device_id, events };
        let message = ControlMessage {
            id: id.clone(),
            command,
        };

        let message_json = serde_json::to_string(&message)?;

        let mut stream = client.stream.lock().await;

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
