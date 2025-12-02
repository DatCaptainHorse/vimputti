use std::time::Duration;
use tokio::time::sleep;
use vimputti::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = VimputtiClient::connect_default().await?;
    println!("Connected to vimputti manager\n");

    // Test Xbox 360 controller
    println!("=== Creating Xbox 360 Controller ===");
    let xbox = client.create_device(ControllerTemplates::xbox360()).await?;
    println!("Created: {}", xbox.event_node());

    xbox.button_press(Button::A);
    xbox.axis(Axis::LeftStickX, 16384);
    xbox.sync().await?;

    sleep(Duration::from_secs(1)).await;
    drop(xbox);
    println!("Destroyed Xbox 360 controller\n");

    sleep(Duration::from_millis(500)).await;

    // Test PS5 controller
    println!("=== Creating PS5 Controller ===");
    let ps5 = client.create_device(ControllerTemplates::ps5()).await?;
    println!("Created: {}", ps5.event_node());

    ps5.button_press(Button::X);
    ps5.axis(Axis::RightStickY, 128);
    ps5.sync().await?;

    sleep(Duration::from_secs(1)).await;
    drop(ps5);
    println!("Destroyed PS5 controller\n");

    sleep(Duration::from_millis(500)).await;

    // Test Switch Pro controller
    println!("=== Creating Switch Pro Controller ===");
    let switch = client
        .create_device(ControllerTemplates::switch_pro())
        .await?;
    println!("Created: {}", switch.event_node());

    switch.button_press(Button::B); // A button on Nintendo layout
    switch.axis(Axis::LeftStickX, -10000);
    switch.sync().await?;

    sleep(Duration::from_secs(1)).await;
    drop(switch);
    println!("Destroyed Switch Pro controller\n");

    sleep(Duration::from_millis(500)).await;

    // Test custom controller using builder
    println!("=== Creating Custom Controller ===");
    let custom_config = ControllerBuilder::new("My Custom Controller")
        .vendor_id(0x1234)
        .product_id(0x5678)
        .bustype(BusType::Usb)
        .face_buttons()
        .shoulder_buttons()
        .menu_buttons()
        .dual_analog_sticks()
        .analog_triggers()
        .build();

    let custom = client.create_device(custom_config).await?;
    println!("Created: {}", custom.event_node());

    custom.button_press(Button::Start);
    custom.axis(Axis::RightStickX, 20000);
    custom.sync().await?;

    sleep(Duration::from_secs(1)).await;
    drop(custom);
    println!("Destroyed custom controller\n");

    println!("All tests completed!");

    Ok(())
}
