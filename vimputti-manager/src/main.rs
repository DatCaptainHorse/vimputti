use clap::Parser;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;
use vimputti::manager::Manager;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Socket path for the manager
    #[arg(short, long)]
    socket: Option<PathBuf>,

    /// Instance number (used to generate socket path)
    #[arg(short, long, default_value = "0")]
    instance: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    // Determine socket path
    let socket_path = if let Some(path) = args.socket {
        path
    } else {
        // Use default path based on instance number
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/run/user/{}/vimputti-{}", uid, args.instance))
    };

    tracing::info!("Starting vimputti manager");
    tracing::info!("Socket path: {}", socket_path.display());

    // Create and run manager
    let mut manager = Manager::new(&socket_path)?;
    manager.run().await?;

    Ok(())
}
