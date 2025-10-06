use std::time::Duration;
use tokio::time::sleep;
use vimputti::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to the manager
    let client = VimputtiClient::connect_default().await?;

    // Ping to verify connection
    client.ping().await?;
    println!("Connected to vimputti manager");

    // Create a simple Xbox-style controller configuration
    let config = ControllerTemplates::xbox360();

    // Create the virtual device
    let device = client.create_device(config).await?;
    println!("Created device: {}", device.event_node());

    // Send some test inputs
    println!("Pressing A button...");
    device.button_press(Button::A);
    sleep(Duration::from_millis(100)).await;
    device.button_release(Button::A);

    println!("Moving left stick...");
    device.axis(Axis::LeftStickX, 16384);
    device.axis(Axis::LeftStickY, -16384);
    sleep(Duration::from_millis(100)).await;

    // Reset to center
    device.axis(Axis::LeftStickX, 0);
    device.axis(Axis::LeftStickY, 0);

    // Manually flush to ensure events are sent
    device.flush().await?;

    println!("Test complete!");

    // Give a moment for cleanup
    sleep(Duration::from_millis(50)).await;

    // Device is automatically destroyed when it goes out of scope

    Ok(())
}
