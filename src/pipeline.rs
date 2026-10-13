use crate::capture::{Backend, CaptureError, CapturedScreen};
use crate::config::Config;
use crate::provider::{
    ChatProvider, OllamaProvider, OpenAIProvider, OpenCodeProvider, ProviderError, StubProvider,
};
use std::path::Path;

#[derive(Debug, PartialEq)]
pub struct AskSuccess {
    pub answer: String,
    pub backend: Option<Backend>,
}

#[derive(Debug, PartialEq)]
pub enum AskError {
    Image(String),
    Capture(CaptureError),
    Provider(ProviderError),
}

pub fn build_provider(config: &Config) -> Result<Box<dyn ChatProvider + Send + Sync>, String> {
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
                    return Err(
                        "OPENAI_API_KEY is not set; export it before using the openai provider"
                            .to_string(),
                    );
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
        other => Err(format!(
            "provider \"{other}\" not yet implemented in milestone 1"
        )),
    }
}

pub fn ask_once(
    question: &str,
    image: Option<&Path>,
    capture: impl FnOnce() -> Result<CapturedScreen, CaptureError>,
    provider: &(dyn ChatProvider + Sync),
) -> Result<AskSuccess, AskError> {
    let (image_bytes, backend) = match image {
        Some(path) => match std::fs::read(path) {
            Ok(bytes) => (bytes, None),
            Err(err) => {
                return Err(AskError::Image(format!(
                    "cannot read image {}: {err}",
                    path.display()
                )));
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

    #[test]
    fn ask_once_captures_screen_and_returns_answer() {
        let screen = CapturedScreen {
            png: vec![1, 2, 3],
            backend: Backend::Portal,
        };
        let provider = StubProvider;
        let outcome = ask_once("what is on my screen?", None, || Ok(screen), &provider).unwrap();
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
            AskError::Image(message) => {
                assert!(message.contains("cannot read image"), "got: {message}");
                assert!(
                    message.contains("/nonexistent/dithus-screen.png"),
                    "got: {message}"
                );
            }
            other => panic!("expected image error, got: {other:?}"),
        }
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
        let err = ask_once("hi", None, || Ok(screen), &Failing).unwrap_err();
        assert_eq!(
            err,
            AskError::Provider(ProviderError("provider blew up".to_string()))
        );
    }

    #[test]
    fn build_provider_stub_answers() {
        let config = Config {
            provider: "stub".to_string(),
            ..Config::default()
        };
        let provider = match build_provider(&config) {
            Ok(provider) => provider,
            Err(err) => panic!("expected stub provider, got error: {err}"),
        };
        assert_eq!(provider.ask("hi", &[1]).unwrap(), "stub answer: hi");
    }

    #[test]
    fn build_provider_unknown_name_is_rejected() {
        let config = Config {
            provider: "pluely".to_string(),
            ..Config::default()
        };
        match build_provider(&config) {
            Ok(_) => panic!("expected unknown-provider error"),
            Err(err) => assert!(err.contains("pluely"), "got: {err}"),
        }
    }

    #[test]
    fn build_provider_is_usable_off_the_cli_thread() {
        let config = Config {
            provider: "stub".to_string(),
            ..Config::default()
        };
        let provider = match build_provider(&config) {
            Ok(provider) => provider,
            Err(err) => panic!("expected stub provider, got error: {err}"),
        };
        let answer = std::thread::spawn(move || provider.ask("hi", &[1]).unwrap())
            .join()
            .unwrap();
        assert_eq!(answer, "stub answer: hi");
    }
}
