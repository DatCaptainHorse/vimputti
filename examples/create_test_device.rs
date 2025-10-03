use std::time::Duration;
use tokio::time::sleep;
use vimputti::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = VimputtiClient::connect_default().await?;

    let device = client.create_device(ControllerTemplates::xbox360()).await?;
    println!("Created device: {}", device.event_node());
    println!("Keeping device alive... Press Ctrl+C to destroy it.");

    // Keep the device alive
    loop {
        sleep(Duration::from_secs(1)).await;
    }
}
