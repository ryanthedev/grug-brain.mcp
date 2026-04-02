use clap::{Parser, Subcommand};
use grug_brain::client::run_stdio;
use grug_brain::server::run_server;
use grug_brain::service_install;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "grug", version, about = "grug-brain memory server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run as MCP stdio client (connects to running server).
    #[arg(long)]
    stdio: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the grug-brain server.
    Serve {
        /// Custom socket path (default: ~/.grug-brain/grug.sock).
        #[arg(long)]
        socket: Option<PathBuf>,

        /// Install as a system service (launchd on macOS, systemd on Linux) and exit.
        #[arg(long)]
        install_service: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.stdio {
        if let Err(e) = run_stdio(None).await {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    match cli.command {
        Some(Commands::Serve {
            socket,
            install_service,
        }) => {
            if install_service {
                if let Err(e) =
                    service_install::install_service(socket.as_deref())
                {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return;
            }
            if let Err(e) = run_server(socket, None, None).await {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        None => {
            eprintln!("Usage: grug serve                Start the grug-brain server");
            eprintln!("       grug serve --install-service  Install as system service");
            eprintln!("       grug --stdio              Run as MCP stdio client");
            eprintln!();
            eprintln!("Run grug --help for more info.");
            std::process::exit(1);
        }
    }
}
