use std::time::Duration;
use tokio::time::sleep;
use vimputti::*;

/// Example demonstrating how to poll for force feedback (rumble) events
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to the manager
    let client = VimputtiClient::connect_default().await?;
    println!("Connected to vimputti manager");

    // Create a controller with force feedback support
    let config = ControllerTemplates::xbox360();
    let device = client.create_device(config).await?;
    println!("Created device: {}", device.event_node());
    println!();

    println!("Virtual controller created!");
    println!("To test rumble:");
    println!("  1. Open the device in another application");
    println!(
        "  2. Send force feedback commands to /dev/input/{}",
        device.event_node()
    );
    println!("  3. Watch this program detect the rumble events");
    println!();
    println!("Polling for rumble events for 30 seconds..");
    println!("Press Ctrl+C to exit");
    println!();

    // Poll for feedback events in a loop
    let start = std::time::Instant::now();
    let mut feedback_count = 0;

    while start.elapsed() < Duration::from_secs(30) {
        // Poll for feedback
        match device.poll_feedback().await? {
            Some(event) => {
                feedback_count += 1;
                match event {
                    FeedbackEvent::Rumble {
                        strong_magnitude,
                        weak_magnitude,
                        duration_ms,
                    } => {
                        println!(
                            "[{}] Rumble: strong={}, weak={}, duration={}ms",
                            feedback_count, strong_magnitude, weak_magnitude, duration_ms
                        );
                    }
                    FeedbackEvent::RumbleStop => {
                        println!("[{}] Rumble Stopped", feedback_count);
                    }
                    FeedbackEvent::Raw { code, value } => {
                        println!(
                            "[{}] Raw Feedback: code=0x{:04x}, value={}",
                            feedback_count, code, value
                        );
                    }
                }
            }
            None => {
                // No feedback available - sleep briefly before polling again
                sleep(Duration::from_millis(50)).await;
            }
        }
    }

    if feedback_count == 0 {
        println!("No feedback events received");
    } else {
        println!("\nTotal feedback events: {}", feedback_count);
    }

    println!("\nCleaning up..");
    drop(device);
    sleep(Duration::from_millis(100)).await;

    Ok(())
}
