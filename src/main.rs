use anyhow::Result;
use shared_runtime_manager::{api, dependency_queue, manager};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "srm", version, about = "Shared Runtime Manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Register {
        app_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        path: String,
    },
    Apps,
    Install { resource: String, #[arg(long)] app: Option<String> },
    Resources,
    Resolve { resource: String, #[arg(long)] app: Option<String> },
    Api { #[command(subcommand)] command: ApiCommand },
    Root,
    ProcessQueue,
    QueueStatus,
}

#[derive(Subcommand)]
enum ApiCommand {
    Add { provider: String, #[arg(long)] name: String, #[arg(long)] endpoint: String, #[arg(long)] model: String },
    Authorize { provider: String, #[arg(long)] app: String },
    SetKey { provider: String },
    List,
    Resolve { provider: String, #[arg(long)] app: String },
    KeyStatus { provider: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Register { app_id, name, path } => {
            manager::register_app_at_path(&app_id, &name, &path)?;
            dependency_queue::enqueue_application(&app_id, &path)
        },
        Command::Apps => manager::list_apps(),
        Command::Install { resource, app } => manager::install(&resource, app.as_deref()),
        Command::Resources => manager::list_resources(),
        Command::Resolve { resource, app } => manager::resolve(&resource, app.as_deref()),
        Command::Api { command } => match command {
            ApiCommand::Add { provider, name, endpoint, model } => api::add(&provider, &name, &endpoint, &model),
            ApiCommand::Authorize { provider, app } => api::authorize(&provider, &app),
            ApiCommand::SetKey { provider } => api::set_key_from_stdin(&provider),
            ApiCommand::List => api::list(),
            ApiCommand::Resolve { provider, app } => api::resolve(&provider, &app),
            ApiCommand::KeyStatus { provider } => api::key_status(&provider),
        },
        Command::ProcessQueue => dependency_queue::process_queue(),
        Command::QueueStatus => {\n            println!("{}", serde_json::to_string_pretty(&dependency_queue::status()?)?);\n            Ok(())\n        },\n        Command::Root => {
            println!("{}", manager::root()?.display());
            Ok(())
        }
    }
}
