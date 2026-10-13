mod capture;
mod config;
mod pipeline;
mod provider;
mod transcribe;

#[cfg(test)]
mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static WRITE_EXEC_LOCK: Mutex<()> = Mutex::new(());

    pub fn write_exec_guard() -> MutexGuard<'static, ()> {
        WRITE_EXEC_LOCK.lock().unwrap()
    }
}

use capture::capture_screenshot;
use clap::{Parser, Subcommand};
use config::load_config;
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
        #[arg(long)]
        repeat: bool,
        #[arg(long, help = "Seconds between repeated asks; implies --repeat")]
        interval: Option<u64>,
    },
    Listen {
        #[arg(long)]
        audio: Option<PathBuf>,
        #[arg(long, default_value_t = 5)]
        secs: u64,
        #[arg(long, default_value = "dithus.toml")]
        config: PathBuf,
    },
}

fn describe_stage(config: &config::Config) -> String {
    match config.provider.as_str() {
        "stub" => "stub".to_string(),
        "opencode" if config.opencode_model.is_empty() => {
            "opencode with default model".to_string()
        }
        "opencode" => format!("opencode with model {}", config.opencode_model),
        "ollama" => format!("ollama with model {}", config.ollama_model),
        "openai" => format!("openai with model {}", config.openai_model),
        other => format!("{other} (unimplemented)"),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Commands::Ask {
            question,
            image,
            config,
            repeat,
            interval,
        } => ask(question, image, config, repeat, interval),
        Commands::Listen {
            audio,
            secs,
            config,
        } => listen(audio, secs, config),
    }
}

fn listen(audio: Option<PathBuf>, secs: u64, config: PathBuf) -> ExitCode {
    let loaded = load_config(&config);
    eprintln!("listening (language {})...", loaded.language);
    let whisper = transcribe::WhisperCli {
        command: transcribe::DEFAULT_TRANSCRIBER.to_string(),
        model: PathBuf::from(&loaded.transcribe_model),
        timeout: std::time::Duration::from_secs(120),
    };
    let staged;
    let wav = match &audio {
        Some(path) => path.as_path(),
        None => {
            staged = std::env::temp_dir().join(format!(
                "dithus-mic-{}-{}.wav",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            if let Err(err) =
                transcribe::record_mic_with_command(transcribe::DEFAULT_RECORDER, secs, &staged)
            {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
            staged.as_path()
        }
    };
    let result = whisper.transcribe_wav_file(wav, &loaded.language);
    if audio.is_none() {
        let _ = std::fs::remove_file(wav);
    }
    match result {
        Ok(transcript) => {
            println!("{transcript}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{}", err.0);
            ExitCode::from(1)
        }
    }
}

fn ask(
    question: String,
    image: Option<PathBuf>,
    config: PathBuf,
    repeat: bool,
    interval: Option<u64>,
) -> ExitCode {
    let loaded = load_config(&config);
    eprintln!("asking {}...", describe_stage(&loaded));

    let period = interval.or(if repeat {
        Some(loaded.capture_interval_secs)
    } else {
        None
    });

    loop {
        match pipeline::run_ask(&loaded, &question, image.as_deref(), capture_screenshot) {
            Ok(outcome) => {
                if let Some(backend) = outcome.backend {
                    eprintln!("captured screen via {}", backend.name());
                }
                println!("{}", outcome.answer);
                match period {
                    Some(secs) => std::thread::sleep(std::time::Duration::from_secs(secs)),
                    None => return ExitCode::SUCCESS,
                }
            }
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::from(match err {
                    pipeline::PipelineError::Build(_) => 2,
                    pipeline::PipelineError::Ask(pipeline::AskError::Image { .. }) => 2,
                    _ => 1,
                });
            }
        }
    }
}
