mod acp;
mod app;
mod audit;
mod complete;
mod config;
mod form;
mod hints;
mod history;
mod input;
mod markdown;
mod mcp;
mod mention;
mod project;
mod replay;
mod setup;
mod shell;
mod shellenv;
mod title;
mod turn;
mod ui;
mod version;
mod wrap;

use clap::Parser;
use std::process::ExitCode;

/// Parolsh: a natural-language-first shell for ACP agents.
#[derive(Parser)]
#[command(version = version::long())]
struct Cli {
    /// Run COMMAND with a plain `bash -c` and exit, without the agent
    #[arg(short = 'c', value_name = "COMMAND")]
    command: Option<String>,

    /// Where plain text goes at start: to the agent, or to the shell
    /// (overrides `input` in config.toml)
    #[arg(long, value_enum, conflicts_with = "command")]
    input: Option<input::Mode>,

    /// Go back to session N of this project, like `#resume N` (see `#sessions`)
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(i64).range(1..),
        conflicts_with_all = ["command", "continue_last"]
    )]
    resume: Option<i64>,

    /// Go back to the last session of this project
    #[arg(long = "continue", conflicts_with = "command")]
    continue_last: bool,

    /// Arguments for COMMAND, available as $0, $1, ...
    #[arg(
        requires = "command",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    args: Vec<String>,

    #[command(subcommand)]
    tool: Option<Tool>,
}

#[derive(clap::Subcommand)]
enum Tool {
    /// The history of sessions as an MCP server on stdio, read-only, for the
    /// agent: Parolsh gives it to the agent itself
    #[command(hide = true)]
    Mcp {
        /// The history database
        #[arg(long)]
        db: std::path::PathBuf,
        /// Only the sessions of this project
        #[arg(long)]
        project: String,
        /// Also return the plain `!command` lines (`commands = "shared"`)
        #[arg(long)]
        commands: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(Tool::Mcp {
        db,
        project,
        commands,
    }) = cli.tool
    {
        let scope = mcp::Scope {
            project: &project,
            commands,
        };
        return match mcp::serve(&db, scope) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("parolsh mcp: {e:#}");
                ExitCode::FAILURE
            }
        };
    }

    if let Some(line) = cli.command {
        return match shell::passthrough(&line, &cli.args).status() {
            Ok(status) => ExitCode::from(shell::exit_code(status) as u8),
            Err(e) => {
                eprintln!("parolsh: {e}");
                ExitCode::from(127)
            }
        };
    }

    // First, while Parolsh has a single thread: it may change the
    // environment. A broken configuration is reported by App::new.
    let notices: Vec<String> = std::env::current_dir()
        .ok()
        .and_then(|cwd| config::Config::load(project::find_root(&cwd).as_deref()).ok())
        // SAFETY: no other thread has been started yet.
        .and_then(|config| unsafe { shellenv::load(config.shell_env, &config.shell[0]) })
        .into_iter()
        .collect();

    let resume = match (cli.resume, cli.continue_last) {
        (Some(session), _) => Some(app::Resume::Session(session)),
        (None, true) => Some(app::Resume::Latest),
        (None, false) => None,
    };

    if let Err(e) = turn::install_interrupt_handler() {
        eprintln!("parolsh: cannot handle Ctrl+C: {e}");
    }

    let result = std::env::current_dir()
        .map_err(anyhow::Error::from)
        .and_then(|cwd| app::App::new(cwd, notices, cli.input, resume))
        .and_then(|mut app| app.run());

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("parolsh: {e:#}");
            ExitCode::FAILURE
        }
    }
}
