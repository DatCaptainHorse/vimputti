use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::sleep;
use vimputti::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Starting udev monitor test...");

    // Connect to udev socket
    let uid = unsafe { libc::getuid() };
    let udev_socket = format!("/run/user/{}/vimputti/udev", uid);

    println!("Connecting to udev socket: {}", udev_socket);
    let stream = UnixStream::connect(&udev_socket).await?;
    let reader = BufReader::new(stream);

    // Spawn task to monitor udev events
    tokio::spawn(async move {
        monitor_udev(reader).await;
    });

    // Give monitor time to connect
    sleep(Duration::from_millis(100)).await;

    // Connect to manager and create a device
    println!("\nConnecting to vimputti manager...");
    let client = VimputtiClient::connect_default().await?;

    println!("Creating virtual controller...");
    let config = DeviceConfig {
        name: "Test Controller".to_string(),
        vendor_id: 0x045e,
        product_id: 0x028e,
        version: 0x0110,
        bustype: BusType::Usb,
        buttons: vec![Button::A, Button::B],
        axes: vec![AxisConfig::new(Axis::LeftStickX, -32768, 32767)],
    };

    let device = client.create_device(config).await?;
    println!("Device created: {}", device.event_node());

    // Wait a bit to see the add event
    sleep(Duration::from_secs(2)).await;

    println!("\nDestroying device...");
    drop(device);

    // Wait to see the remove event
    sleep(Duration::from_secs(2)).await;

    println!("\nTest complete!");

    Ok(())
}

async fn monitor_udev(mut reader: BufReader<UnixStream>) {
    println!("Udev monitor started, waiting for events...\n");

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                if line.trim().is_empty() {
                    println!("--- End of udev event ---\n");
                } else {
                    print!("{}", line);
                }
            }
            Err(e) => {
                eprintln!("Error reading udev event: {}", e);
                break;
            }
        }
    }
}
