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
    let config = DeviceConfig {
        name: "Virtual Xbox Controller".to_string(),
        vendor_id: 0x045e,  // Microsoft
        product_id: 0x028e, // Xbox 360 Controller
        version: 0x0110,
        bustype: BusType::Usb,
        buttons: vec![
            Button::A,
            Button::B,
            Button::X,
            Button::Y,
            Button::LeftBumper,
            Button::RightBumper,
            Button::Start,
            Button::Select,
            Button::Guide,
        ],
        axes: vec![
            AxisConfig::new(Axis::LeftStickX, -32768, 32767),
            AxisConfig::new(Axis::LeftStickY, -32768, 32767),
            AxisConfig::new(Axis::RightStickX, -32768, 32767),
            AxisConfig::new(Axis::RightStickY, -32768, 32767),
            AxisConfig::new(Axis::LeftTrigger, 0, 255),
            AxisConfig::new(Axis::RightTrigger, 0, 255),
        ],
    };

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
