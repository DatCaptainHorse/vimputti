use clap::{Arg, Command};
use manager::InputManager;

mod manager;
mod protocol;
mod shim;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = Command::new("vimputti-manager")
        .version("0.1.0")
        .about("Manager for the vimputti input emulation system")
        .arg(
            Arg::new("socket")
                .short('s')
                .long("socket")
                .value_name("PATH")
                .help("Path to the manager socket"),
        )
        .get_matches();

    // Get the socket path from command line argument or use default
    let socket_path = if let Some(path) = matches.get_one::<String>("socket") {
        path.clone()
    } else {
        let uid = unsafe { libc::getuid() };
        format!("/run/user/{}/vimputti-0", uid)
    };

    // Create and run the input manager
    let mut manager = InputManager::new(socket_path);
    manager.run().await?;

    Ok(())
}
