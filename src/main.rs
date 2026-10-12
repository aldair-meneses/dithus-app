mod config;
mod provider;

use clap::{Parser, Subcommand};
use config::load_config;
use provider::{ChatProvider, StubProvider};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Ask {
        #[arg(long)]
        question: String,
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long, default_value = "dithus.toml")]
        config: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Commands::Ask {
        question,
        image,
        config,
    } = cli.command;

    let loaded = load_config(&config);
    let provider: Box<dyn ChatProvider> = match loaded.provider.as_str() {
        "stub" => Box::new(StubProvider),
        other => {
            eprintln!("provider \"{other}\" not yet implemented in milestone 1");
            return ExitCode::from(2);
        }
    };

    let image_bytes = match image {
        Some(path) => match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("cannot read image {}: {err}", path.display());
                return ExitCode::from(2);
            }
        },
        None => Vec::new(),
    };

    match provider.ask(&question, &image_bytes) {
        Ok(answer) => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("provider error: {}", err.0);
            ExitCode::from(1)
        }
    }
}
