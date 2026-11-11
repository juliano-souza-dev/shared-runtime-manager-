use anyhow::Result;
use shared_runtime_manager::{api, manager};
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
        app_id: String,
        #[arg(long)]
        name: String,
    },
    /// List registered applications.
    Apps,
    /// Install or update a managed shared resource.
    Install {
        resource: String,
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
    /// Manage globally shared API configurations and credentials.
    Api {
        #[command(subcommand)]
        command: ApiCommand,
    },
    /// Print the shared runtime root directory.
    Root,
}

#[derive(Subcommand)]
enum ApiCommand {
    /// Create or update a global API configuration.
    Add {
        provider: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        model: String,
    },
    /// Authorize an application to use a global API configuration.
    Authorize {
        provider: String,
        #[arg(long)]
        app: String,
    },
    /// Set the API secret. Reads the key from stdin, never from command arguments.
    SetKey {
        provider: String,
    },
    /// List API configurations without exposing secrets.
    List,
    /// Resolve an API configuration for an authorized application.
    Resolve {
        provider: String,
        #[arg(long)]
        app: String,
    },
    /// Check whether a secret exists in the operating-system credential store.
    KeyStatus {
        provider: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Register { app_id, name } => manager::register_app(&app_id, &name),
        Command::Apps => manager::list_apps(),
        Command::Install { resource, app } => manager::install(&resource, app.as_deref()),
        Command::Resources => manager::list_resources(),
        Command::Resolve { resource, app } => manager::resolve(&resource, app.as_deref()),
        Command::Api { command } => match command {
            ApiCommand::Add { provider, name, endpoint, model } =>
                api::add(&provider, &name, &endpoint, &model),
            ApiCommand::Authorize { provider, app } => api::authorize(&provider, &app),
            ApiCommand::SetKey { provider } => api::set_key_from_stdin(&provider),
            ApiCommand::List => api::list(),
            ApiCommand::Resolve { provider, app } => api::resolve(&provider, &app),
            ApiCommand::KeyStatus { provider } => api::key_status(&provider),
        },
        Command::Root => {
            println!("{}", manager::root()?.display());
            Ok(())
        }
    }
}
