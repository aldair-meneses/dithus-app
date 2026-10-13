use crate::capture::{Backend, CaptureError, CapturedScreen};
use crate::config::Config;
use crate::provider::{
    ChatProvider, OllamaProvider, OpenAIProvider, OpenCodeProvider, ProviderError, StubProvider,
};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub struct AskSuccess {
    pub answer: String,
    pub backend: Option<Backend>,
}

#[derive(Debug, PartialEq)]
pub enum AskError {
    Image { path: PathBuf, reason: String },
    Capture(CaptureError),
    Provider(ProviderError),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AskError::Image { path, reason } => {
                write!(f, "cannot read image {}: {reason}", path.display())
            }
            AskError::Capture(err) => write!(f, "{}", err.0),
            AskError::Provider(err) => write!(f, "provider error: {}", err.0),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum BuildError {
    UnknownProvider(String),
    MissingApiKey,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::UnknownProvider(name) => {
                write!(f, "provider \"{name}\" not yet implemented in milestone 1")
            }
            BuildError::MissingApiKey => write!(
                f,
                "OPENAI_API_KEY is not set; export it before using the openai provider"
            ),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum PipelineError {
    Build(BuildError),
    Ask(AskError),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Build(err) => write!(f, "{err}"),
            PipelineError::Ask(err) => write!(f, "{err}"),
        }
    }
}

pub fn build_provider(config: &Config) -> Result<Box<dyn ChatProvider + Send + Sync>, BuildError> {
    match config.provider.as_str() {
        "stub" => Ok(Box::new(StubProvider)),
        "opencode" => Ok(Box::new(OpenCodeProvider {
            command: "opencode".to_string(),
            model: config.opencode_model.clone(),
            timeout: std::time::Duration::from_secs(300),
        })),
        "ollama" => Ok(Box::new(OllamaProvider {
            base_url: std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".to_string()),
            model: config.ollama_model.clone(),
            timeout: std::time::Duration::from_secs(120),
        })),
        "openai" => {
            let api_key = match std::env::var("OPENAI_API_KEY") {
                Ok(key) if !key.is_empty() => key,
                _ => {
                    return Err(BuildError::MissingApiKey);
                }
            };
            Ok(Box::new(OpenAIProvider {
                base_url: std::env::var("OPENAI_BASE_URL")
                    .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
                model: config.openai_model.clone(),
                api_key,
                timeout: std::time::Duration::from_secs(120),
            }))
        }
        other => Err(BuildError::UnknownProvider(other.to_string())),
    }
}

pub fn run_ask(
    config: &Config,
    question: &str,
    image: Option<&Path>,
    capture: impl FnMut() -> Result<CapturedScreen, CaptureError> + Send,
) -> Result<AskSuccess, PipelineError> {
    let provider = build_provider(config).map_err(PipelineError::Build)?;
    ask_once(question, image, capture, provider.as_ref()).map_err(PipelineError::Ask)
}

pub fn ask_once(
    question: &str,
    image: Option<&Path>,
    mut capture: impl FnMut() -> Result<CapturedScreen, CaptureError> + Send,
    provider: &(dyn ChatProvider + Sync),
) -> Result<AskSuccess, AskError> {
    let (image_bytes, backend) = match image {
        Some(path) => match std::fs::read(path) {
            Ok(bytes) => (bytes, None),
            Err(err) => {
                return Err(AskError::Image {
                    path: path.to_path_buf(),
                    reason: err.to_string(),
                });
            }
        },
        None => match capture() {
            Ok(screen) => (screen.png, Some(screen.backend)),
            Err(err) => return Err(AskError::Capture(err)),
        },
    };
    match provider.ask(question, &image_bytes) {
        Ok(answer) => Ok(AskSuccess { answer, backend }),
        Err(err) => Err(AskError::Provider(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::StubProvider;

    fn stub_config() -> Config {
        Config {
            provider: "stub".to_string(),
            ..Config::default()
        }
    }

    fn build_ok(config: &Config) -> Box<dyn ChatProvider + Send + Sync> {
        match build_provider(config) {
            Ok(provider) => provider,
            Err(err) => panic!("expected provider, got error: {err}"),
        }
    }

    #[test]
    fn ask_once_captures_screen_and_returns_answer() {
        let screen = CapturedScreen {
            png: vec![1, 2, 3],
            backend: Backend::Portal,
        };
        let provider = StubProvider;
        let outcome = ask_once(
            "what is on my screen?",
            None,
            || Ok(screen.clone()),
            &provider,
        )
        .unwrap();
        assert_eq!(outcome.answer, "stub answer: what is on my screen?");
        assert_eq!(outcome.backend, Some(Backend::Portal));
    }

    #[test]
    fn ask_once_reads_image_file_and_skips_capture() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("dithus-pipe-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("screen.png");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&[4, 5, 6]).unwrap();
        drop(file);
        struct Recording;
        impl ChatProvider for Recording {
            fn ask(&self, question: &str, image: &[u8]) -> Result<String, ProviderError> {
                assert_eq!(question, "what is in the image?");
                assert_eq!(image, &[4, 5, 6]);
                Ok("saw it".to_string())
            }
        }
        let outcome = ask_once(
            "what is in the image?",
            Some(&path),
            || panic!("capture must not run when an image file is given"),
            &Recording,
        )
        .unwrap();
        assert_eq!(outcome.answer, "saw it");
        assert_eq!(outcome.backend, None);
    }

    #[test]
    fn ask_once_missing_image_file_reports_path() {
        let provider = StubProvider;
        let missing = std::path::PathBuf::from("/nonexistent/dithus-screen.png");
        let err = ask_once("hi", Some(&missing), || panic!("no capture"), &provider).unwrap_err();
        match err {
            AskError::Image { path, reason } => {
                assert_eq!(path, missing);
                assert!(!reason.is_empty(), "expected IO reason, got empty");
            }
            other => panic!("expected image error, got: {other:?}"),
        }
    }

    #[test]
    fn ask_error_messages_match_cli_output() {
        assert_eq!(
            AskError::Image {
                path: PathBuf::from("/tmp/shot.png"),
                reason: "gone".to_string(),
            }
            .to_string(),
            "cannot read image /tmp/shot.png: gone"
        );
        assert_eq!(
            AskError::Capture(CaptureError("portal down".to_string())).to_string(),
            "portal down"
        );
        assert_eq!(
            AskError::Provider(ProviderError("blew up".to_string())).to_string(),
            "provider error: blew up"
        );
    }

    #[test]
    fn ask_once_capture_failure_reaches_caller() {
        let provider = StubProvider;
        let err = ask_once(
            "hi",
            None,
            || {
                Err(CaptureError(
                    "screen capture failed (portal: down; grim: down). On Sway.".to_string(),
                ))
            },
            &provider,
        )
        .unwrap_err();
        assert_eq!(
            err,
            AskError::Capture(CaptureError(
                "screen capture failed (portal: down; grim: down). On Sway.".to_string()
            ))
        );
    }

    #[test]
    fn ask_once_provider_failure_reaches_caller() {
        struct Failing;
        impl ChatProvider for Failing {
            fn ask(&self, _question: &str, _image: &[u8]) -> Result<String, ProviderError> {
                Err(ProviderError("provider blew up".to_string()))
            }
        }
        let screen = CapturedScreen {
            png: vec![1],
            backend: Backend::Grim,
        };
        let err = ask_once("hi", None, || Ok(screen.clone()), &Failing).unwrap_err();
        assert_eq!(
            err,
            AskError::Provider(ProviderError("provider blew up".to_string()))
        );
    }

    #[test]
    fn build_provider_stub_answers() {
        let provider = build_ok(&stub_config());
        assert_eq!(provider.ask("hi", &[1]).unwrap(), "stub answer: hi");
    }

    #[test]
    fn build_error_messages_match_cli_output() {
        assert_eq!(
            BuildError::UnknownProvider("pluely".to_string()).to_string(),
            "provider \"pluely\" not yet implemented in milestone 1"
        );
        assert_eq!(
            BuildError::MissingApiKey.to_string(),
            "OPENAI_API_KEY is not set; export it before using the openai provider"
        );
    }

    #[test]
    fn build_provider_unknown_name_is_rejected() {
        let config = Config {
            provider: "pluely".to_string(),
            ..Config::default()
        };
        match build_provider(&config) {
            Ok(_) => panic!("expected unknown-provider error"),
            Err(err) => assert_eq!(err, BuildError::UnknownProvider("pluely".to_string())),
        }
    }

    #[test]
    fn run_ask_combines_config_capture_and_provider() {
        let config = stub_config();
        let screen = CapturedScreen {
            png: vec![1, 2, 3],
            backend: Backend::Grim,
        };
        let outcome = run_ask(
            &config,
            "what is on my screen?",
            None,
            || Ok(screen.clone()),
        )
        .unwrap();
        assert_eq!(outcome.answer, "stub answer: what is on my screen?");
        assert_eq!(outcome.backend, Some(Backend::Grim));
    }

    #[test]
    fn run_ask_surfaces_build_failure() {
        let config = Config {
            provider: "pluely".to_string(),
            ..Config::default()
        };
        let err = run_ask(&config, "hi", None, || panic!("no capture")).unwrap_err();
        assert_eq!(
            err,
            PipelineError::Build(BuildError::UnknownProvider("pluely".to_string()))
        );
    }

    #[test]
    fn run_ask_runs_off_the_calling_thread() {
        let config = stub_config();
        let screen = CapturedScreen {
            png: vec![1, 2, 3],
            backend: Backend::Portal,
        };
        let outcome = std::thread::scope(|s| {
            s.spawn(|| run_ask(&config, "hi?", None, || Ok(screen.clone())))
                .join()
                .unwrap()
        })
        .unwrap();
        assert_eq!(outcome.answer, "stub answer: hi?");
        assert_eq!(outcome.backend, Some(Backend::Portal));
    }
}
