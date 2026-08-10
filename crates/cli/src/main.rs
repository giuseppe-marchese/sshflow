mod commands;
mod ipc_client;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "sshflow",
    version,
    about = "An intelligent, zero-config layer on top of OpenSSH: automatic connection reuse, health monitoring and metrics."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Connect to a host, transparently reusing (or creating) a multiplexed connection.
    Ssh {
        /// Destination, e.g. `user@host` or `host:2222`.
        target: String,
        /// Extra arguments passed straight through to `ssh` (e.g. a remote command).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Show daemon status (uptime, active connections, plugins).
    Status,
    /// Show connection metrics (today's totals, time saved, most used hosts).
    Stats,
    /// Run environment diagnostics (ssh binary, permissions, config).
    Doctor,
    /// List currently managed (multiplexed) connections.
    Connections,
    /// Show the daemon's log file.
    Logs {
        /// Number of trailing lines to show.
        #[arg(long, default_value_t = 50)]
        lines: usize,
    },
    /// Inspect or edit the configuration file.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Print the CLI version.
    Version,
    /// Check for a newer release.
    Update,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the resolved config file contents.
    Show,
    /// Print the config file path.
    Path,
    /// Open the config file in `$EDITOR`.
    Edit,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Ssh { target, args } => commands::ssh(&target, args),
        Commands::Status => commands::status(),
        Commands::Stats => commands::stats(),
        Commands::Doctor => commands::doctor(),
        Commands::Connections => commands::connections(),
        Commands::Logs { lines } => commands::logs(lines),
        Commands::Config { action } => match action {
            ConfigAction::Show => commands::config_show(),
            ConfigAction::Path => commands::config_path(),
            ConfigAction::Edit => commands::config_edit(),
        },
        Commands::Version => {
            commands::version();
            Ok(())
        }
        Commands::Update => {
            commands::update();
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("sshflow: error: {e:#}");
        std::process::exit(1);
    }
}
