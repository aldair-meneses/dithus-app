mod capture;
mod config;
mod provider;

use capture::capture_screenshot;
use clap::{Parser, Subcommand};
use config::load_config;
use provider::{ChatProvider, OllamaProvider, OpenAIProvider, OpenCodeProvider, StubProvider};
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
        "opencode" => Box::new(OpenCodeProvider {
            command: "opencode".to_string(),
            model: loaded.opencode_model.clone(),
            timeout: std::time::Duration::from_secs(300),
        }),
        "ollama" => Box::new(OllamaProvider {
            base_url: std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".to_string()),
            model: loaded.ollama_model.clone(),
            timeout: std::time::Duration::from_secs(120),
        }),
        "openai" => {
            let api_key = match std::env::var("OPENAI_API_KEY") {
                Ok(key) if !key.is_empty() => key,
                _ => {
                    eprintln!("OPENAI_API_KEY is not set; export it before using the openai provider");
                    return ExitCode::from(2);
                }
            };
            Box::new(OpenAIProvider {
                base_url: std::env::var("OPENAI_BASE_URL")
                    .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
                model: loaded.openai_model.clone(),
                api_key,
                timeout: std::time::Duration::from_secs(120),
            })
        }
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
        None => match capture_screenshot() {
            Ok(screen) => {
                eprintln!("captured screen via {}", screen.backend.name());
                screen.png
            }
            Err(err) => {
                eprintln!("{}", err.0);
                return ExitCode::from(1);
            }
        },
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
