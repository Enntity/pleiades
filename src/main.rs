// SPDX-License-Identifier: AGPL-3.0-only
use clap::{Parser, Subcommand};
use shrug::cmd::{self, Globals};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Machine-readable output.
    #[arg(long, global = true)]
    json: bool,

    /// Explicit ssh destinations for the peers (skip discovery).
    #[arg(long, global = true, value_delimiter = ',')]
    hosts: Option<Vec<String>>,

    /// ssh user on the peers (default: the local user).
    #[arg(long, global = true)]
    ssh_user: Option<String>,

    /// Catalog release id to use instead of the default.
    #[arg(long, global = true)]
    model: Option<String>,

    /// Accept non-commercial weight licenses without prompting.
    #[arg(long, global = true)]
    accept_noncommercial: bool,

    /// Skip the remote catalog fetch.
    #[arg(long, global = true)]
    offline: bool,

    /// Downgrade the missing clock cap from a blocker to a warning.
    #[arg(long, global = true)]
    no_clock_cap: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Probe, discover and print the plan (the default; read-only).
    Plan,
    /// Preview the plan, or with --go execute it.
    Up {
        /// Actually install, start and verify.
        #[arg(long)]
        go: bool,
        /// Force a full relaunch even if the release is already running.
        #[arg(long)]
        restart: bool,
        /// Gateway listen address stored in config.json (default 127.0.0.1:8145).
        #[arg(long)]
        listen: Option<String>,
    },
    /// List catalog releases and whether each fits this cluster.
    Models,
    /// Running release, pins, containers, memory, clocks and KV metrics.
    Status,
    /// Engine logs on a rank.
    Logs {
        /// Which rank (default 0).
        #[arg(long, default_value_t = 0)]
        rank: usize,
        /// Follow the log.
        #[arg(short = 'f', long)]
        follow: bool,
    },
    /// Remove the containers recorded in state, plus the gateway.
    Down,
    /// Plan or execute a return to the previous release.
    Rollback {
        #[arg(long)]
        go: bool,
    },
    /// Install the GPU clock-cap service on every rank (sudo, needs a TTY).
    ClockCap {
        #[arg(long)]
        go: bool,
    },
    /// Quick benchmark next to the release's published numbers.
    Bench {
        #[arg(long, default_value_t = 2)]
        reps: usize,
    },
    /// Client config for claude-code, codex, opencode or env.
    Connect {
        client: String,
        /// Use the rank-0 LAN address and API key instead of loopback.
        #[arg(long)]
        remote: bool,
        /// Write the config instead of printing it.
        #[arg(long)]
        go: bool,
    },
    /// The HTTP gateway; started detached by `up`. Not for users.
    #[command(hide = true)]
    Serve,
    /// Catalog development commands.
    #[command(hide = true)]
    Catalog {
        #[command(subcommand)]
        cmd: CatalogCmd,
    },
}

#[derive(Subcommand)]
enum CatalogCmd {
    /// Regenerate catalog/catalog.json from releases.json + launchers.
    Build {
        /// Fail if the committed file is stale.
        #[arg(long)]
        check: bool,
    },
    /// Sign catalog/catalog.json with the key in an env var (base64).
    Sign {
        #[arg(long)]
        key_env: String,
    },
    /// Verify FILE against a base64 signature or a signature file.
    Verify { file: PathBuf, sig: String },
    /// Generate a catalog keypair into DIR.
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
}

fn globals(cli: &Cli) -> Globals {
    Globals {
        json: cli.json,
        hosts: cli.hosts.clone(),
        ssh_user: cli
            .ssh_user
            .clone()
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_default(),
        model: cli.model.clone(),
        accept_noncommercial: cli.accept_noncommercial,
        offline: cli.offline,
        no_clock_cap: cli.no_clock_cap,
    }
}

fn run() -> anyhow::Result<i32> {
    let cli = Cli::parse();
    let g = globals(&cli);
    let cwd = std::env::current_dir()?;
    match &cli.command {
        None | Some(Cmd::Plan) => cmd::cluster::plan_cmd(&g),
        Some(Cmd::Up {
            go,
            restart,
            listen,
        }) => cmd::cluster::up(&g, *go, *restart, listen.as_deref()),
        Some(Cmd::Models) => cmd::inspect::models(&g),
        Some(Cmd::Status) => cmd::inspect::status(&g),
        Some(Cmd::Logs { rank, follow }) => cmd::inspect::logs(&g, *rank, *follow),
        Some(Cmd::Down) => cmd::cluster::down(&g),
        Some(Cmd::Rollback { go }) => cmd::cluster::rollback(&g, *go),
        Some(Cmd::Serve) => shrug::serve::run().map(|_| 0),
        Some(Cmd::Bench { reps }) => shrug::bench::run(*reps),
        Some(Cmd::Connect { client, remote, go }) => shrug::connect::run(client, *remote, *go),
        Some(Cmd::ClockCap { go }) => cmd::cluster::clock_cap(&g, *go),
        Some(Cmd::Catalog { cmd }) => match cmd {
            CatalogCmd::Build { check } => cmd::catalog_dev::build(&cwd, *check),
            CatalogCmd::Sign { key_env } => cmd::catalog_dev::sign(&cwd, key_env),
            CatalogCmd::Verify { file, sig } => cmd::catalog_dev::verify(file, sig),
            CatalogCmd::Keygen { out } => cmd::catalog_dev::keygen(out),
        },
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("shrug: {e:#}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
