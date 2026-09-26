mod acp;
mod app;
mod config;
mod form;
mod hints;
mod input;
mod markdown;
mod project;
mod setup;
mod shell;
mod turn;
mod ui;

use clap::Parser;
use std::process::ExitCode;

/// Parolsh: a natural-language-first shell for ACP agents.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Run COMMAND with a plain `bash -c` and exit, without the agent
    #[arg(short = 'c', value_name = "COMMAND")]
    command: Option<String>,

    /// Arguments for COMMAND, available as $0, $1, ...
    #[arg(
        requires = "command",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    args: Vec<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(line) = cli.command {
        return match shell::passthrough(&line, &cli.args).status() {
            Ok(status) => ExitCode::from(shell::exit_code(status) as u8),
            Err(e) => {
                eprintln!("parolsh: {e}");
                ExitCode::from(127)
            }
        };
    }

    if let Err(e) = turn::install_interrupt_handler() {
        eprintln!("parolsh: cannot handle Ctrl+C: {e}");
    }

    let result = std::env::current_dir()
        .map_err(anyhow::Error::from)
        .and_then(app::App::new)
        .and_then(|mut app| app.run());

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("parolsh: {e:#}");
            ExitCode::FAILURE
        }
    }
}
