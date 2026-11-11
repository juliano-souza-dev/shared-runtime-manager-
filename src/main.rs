mod manager;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "srm", version, about = "Shared Runtime Manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Register an application that uses shared dependencies.
    Register {
        /// Stable application identifier, e.g. ttk-media.
        app_id: String,
        /// Human-readable application name.
        #[arg(long)]
        name: String,
    },
    /// List registered applications.
    Apps,
    /// Install or update a managed shared resource.
    Install {
        /// Resource identifier. Currently supported: ffmpeg.
        resource: String,
        /// Application that should be linked to this resource.
        #[arg(long)]
        app: Option<String>,
    },
    /// List installed shared resources.
    Resources,
    /// Resolve the installation path of a resource for an application.
    Resolve {
        resource: String,
        #[arg(long)]
        app: Option<String>,
    },
    /// Print the shared runtime root directory.
    Root,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Register { app_id, name } => manager::register_app(&app_id, &name),
        Command::Apps => manager::list_apps(),
        Command::Install { resource, app } => manager::install(&resource, app.as_deref()),
        Command::Resources => manager::list_resources(),
        Command::Resolve { resource, app } => manager::resolve(&resource, app.as_deref()),
        Command::Root => {
            println!("{}", manager::root()?.display());
            Ok(())
        }
    }
}
