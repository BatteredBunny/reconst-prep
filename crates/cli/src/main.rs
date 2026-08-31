// A bare invocation runs the dataset pipeline; subcommands do everything else.

mod cli;
mod config;
mod out;
mod prep;
mod profiles;

use clap::{CommandFactory, Parser};

use crate::cli::{Cli, Command};

fn main() {
    install_logger();
    let cli = Cli::parse();

    match cli.command {
        Some(command) => run_subcommand(command),
        None => prep::run(cli.prep),
    }
}

fn run_subcommand(command: Command) {
    match command {
        Command::Gui {
            inputs,
            profile,
            out,
        } => {
            let start = reconst_prep_gui::Start {
                inputs,
                profile,
                out_dir: out,
            };
            if let Err(e) = reconst_prep_gui::run(start) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Command::Profiles { what } => exit_on_err(profiles::run(what)),
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
        }
    }
}

fn exit_on_err(result: anyhow::Result<()>) {
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// Everything but our crates stays at `warn`, because winit and wgpu at `info` bury it. `RUST_LOG` overrides the lot.
fn install_logger() {
    let default = "warn,reconst_prep=info,reconst_prep_gui=info,reconst_prep_core=info";
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default))
        .format_timestamp_millis()
        .format_target(false)
        .init();
}
