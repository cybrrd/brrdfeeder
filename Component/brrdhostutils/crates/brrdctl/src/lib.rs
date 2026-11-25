use brrd_core::Snapshot;
use clap::{ColorChoice, CommandFactory, Parser, Subcommand, error::ErrorKind};
use std::{
    ffi::OsString,
    io::{self, Write},
};

#[derive(Parser)]
#[command(name = "brrdctl", version, about = "Read-only system information", disable_help_subcommand = true, color = ColorChoice::Never)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Show system information
    Status {
        /// Report CPU, memory and disk information
        #[arg(long)]
        system: bool,
    },
}

/// Parse the request and write one report. The measurement closure is called
/// only for a valid report request; tests supply the same fixed data as the TUI.
pub fn run(
    args: impl IntoIterator<Item = impl Into<OsString>>,
    measure: impl FnOnce() -> Snapshot,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> io::Result<u8> {
    let mut args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if args.len() == 1 {
        args.push("--help".into());
    }
    let parsed = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr() {
                write!(errors, "{error}")?;
                writeln!(errors, "For valid choices, run brrdctl --help.")?;
                return Ok(2);
            }
            write!(output, "{error}")?;
            return Ok(0);
        }
    };
    match parsed.command {
        Commands::Status { system: false } => {
            let mut command = Cli::command();
            command.build();
            let error = command
                .find_subcommand_mut("status")
                .expect("status command is defined")
                .error(
                    ErrorKind::MissingRequiredArgument,
                    "a report must be chosen; use --system (see brrdctl status --help)",
                );
            write!(errors, "{error}")?;
            Ok(2)
        }
        Commands::Status { system: true } => {
            for line in measure().report_lines() {
                writeln!(output, "{line}")?;
            }
            Ok(0)
        }
    }
}
