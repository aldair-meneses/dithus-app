pub trait ChatProvider {
    fn ask(&self, question: &str, image: &[u8]) -> Result<String, ProviderError>;
}

#[derive(Debug, PartialEq)]
pub struct ProviderError(pub String);

pub struct StubProvider;

impl ChatProvider for StubProvider {
    fn ask(&self, question: &str, _image: &[u8]) -> Result<String, ProviderError> {
        Ok(format!("stub answer: {question}"))
    }
}

pub struct OpenCodeProvider {
    pub command: String,
    pub model: String,
    pub timeout: std::time::Duration,
}

impl ChatProvider for OpenCodeProvider {
    fn ask(&self, question: &str, image: &[u8]) -> Result<String, ProviderError> {
        if image.is_empty() {
            return run_opencode(&self.command, &self.model, question, None, self.timeout);
        }
        let path = std::env::temp_dir().join(format!(
            "dithus-opencode-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, image)
            .map_err(|err| ProviderError(format!("cannot stage screenshot for opencode: {err}")))?;
        let answer = run_opencode(&self.command, &self.model, question, Some(&path), self.timeout);
        let _ = std::fs::remove_file(&path);
        answer
    }
}

fn run_opencode(
    command: &str,
    model: &str,
    question: &str,
    image_file: Option<&std::path::Path>,
    timeout: std::time::Duration,
) -> Result<String, ProviderError> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use wait_timeout::ChildExt;

    let mut args = vec!["run".to_string(), "--format".to_string(), "json".to_string()];
    if !model.is_empty() {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    if let Some(path) = image_file {
        args.push("--file".to_string());
        args.push(path.to_str().unwrap().to_string());
    }
    args.push(question.to_string());

    let mut child = Command::new(command)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            ProviderError(format!(
                "opencode failed to start ({command}): {err}. Is opencode installed and on PATH?"
            ))
        })?;

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(ref mut pipe) = stdout {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    });
    let err_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(ref mut pipe) = stderr {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    });

    match child
        .wait_timeout(timeout)
        .map_err(|err| ProviderError(format!("opencode wait failed: {err}")))?
    {
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(ProviderError(format!(
                "opencode timed out after {}s",
                timeout.as_secs()
            )))
        }
        Some(status) => {
            let stdout_bytes = out_reader.join().unwrap_or_default();
            let stderr_bytes = err_reader.join().unwrap_or_default();
            if !status.success() {
                let stderr_text = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
                let stdout_text = String::from_utf8_lossy(&stdout_bytes).trim().to_string();
                let mut message = format!("opencode exited with {status}: {stderr_text}");
                if !stdout_text.is_empty() {
                    message.push_str(&format!(" | output was: {stdout_text}"));
                }
                return Err(ProviderError(message));
            }
            parse_answer(&stdout_bytes)
        }
    }
}

fn parse_answer(output: &[u8]) -> Result<String, ProviderError> {
    let text = String::from_utf8_lossy(output);
    let mut answer = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(line).map_err(|err| {
            ProviderError(format!("opencode printed non-JSON output: {err}"))
        })?;
        if event.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(chunk) = event
                .get("part")
                .and_then(|part| part.get("text"))
                .and_then(|t| t.as_str())
            {
                answer.push_str(chunk);
            }
        }
        if event.get("type").and_then(|t| t.as_str()) == Some("error") {
            let message = event
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown provider error");
            return Err(ProviderError(format!("opencode reported an error: {message}")));
        }
    }
    if answer.is_empty() {
        return Err(ProviderError(
            "opencode returned no answer text".to_string(),
        ));
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_canned_answer() {
        let provider = StubProvider;
        let answer = provider.ask("what is on my screen?", &[1, 2, 3]).unwrap();
        assert_eq!(answer, "stub answer: what is on my screen?");
    }

    #[test]
    fn opencode_concatenates_text_events_into_answer() {
        let dir = fake_bin(
            "dithus-oc-answer",
            "opencode",
            concat!(
                "#!/bin/sh\n",
                "echo \"$@\" > \"$(dirname \"$0\")/argv\"\n",
                "printf '%s\\n' ",
                "'{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"chunk-one \"}}' ",
                "'{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"chunk-two\"}}'\n",
            ),
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: "test-model".to_string(),
            timeout: std::time::Duration::from_secs(5),
        };
        let answer = provider.ask("what is on my screen?", &[]).unwrap();
        assert_eq!(answer, "chunk-one chunk-two");
    }

    #[test]
    fn opencode_passes_format_model_and_question() {
        let dir = fake_bin(
            "dithus-oc-argv",
            "opencode",
            concat!(
                "#!/bin/sh\n",
                "echo \"$@\" > \"$(dirname \"$0\")/argv\"\n",
                "printf '%s\\n' '{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"hi\"}}'\n",
            ),
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: "test-model".to_string(),
            timeout: std::time::Duration::from_secs(5),
        };
        provider.ask("what is on my screen?", &[]).unwrap();
        let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
        for expected in [
            "run",
            "--format json",
            "--model test-model",
            "what is on my screen?",
        ] {
            assert!(
                argv.contains(expected),
                "expected argv to contain {expected:?}, got: {argv:?}"
            );
        }
    }

    #[test]
    fn opencode_omits_model_flag_when_unset() {
        let dir = fake_bin(
            "dithus-oc-nomodel",
            "opencode",
            concat!(
                "#!/bin/sh\n",
                "echo \"$@\" > \"$(dirname \"$0\")/argv\"\n",
                "printf '%s\\n' '{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"hi\"}}'\n",
            ),
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        provider.ask("hi", &[]).unwrap();
        let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
        assert!(
            !argv.contains("--model"),
            "expected no --model flag, got: {argv:?}"
        );
    }

    #[test]
    fn opencode_delivers_image_bytes_through_attached_file() {
        let dir = fake_bin(
            "dithus-oc-image",
            "opencode",
            concat!(
                "#!/bin/sh\n",
                "echo \"$@\" > \"$(dirname \"$0\")/argv\"\n",
                "prev=\"\"\n",
                "for a in \"$@\"; do\n",
                "if [ \"$prev\" = \"--file\" ]; then cp \"$a\" \"$(dirname \"$0\")/attached.png\"; fi\n",
                "prev=\"$a\"\n",
                "done\n",
                "printf '%s\\n' '{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"saw it\"}}'\n",
            ),
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        let answer = provider.ask("what is in the image?", &[4, 5, 6]).unwrap();
        assert_eq!(answer, "saw it");
        let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
        assert!(
            argv.contains("--file"),
            "expected --file flag, got: {argv:?}"
        );
        assert_eq!(std::fs::read(dir.join("attached.png")).unwrap(), vec![4, 5, 6]);
    }

    #[test]
    fn opencode_skips_file_flag_without_image() {
        let dir = fake_bin(
            "dithus-oc-noimage",
            "opencode",
            concat!(
                "#!/bin/sh\n",
                "echo \"$@\" > \"$(dirname \"$0\")/argv\"\n",
                "printf '%s\\n' '{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"hi\"}}'\n",
            ),
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        provider.ask("hi", &[]).unwrap();
        let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
        assert!(
            !argv.contains("--file"),
            "expected no --file flag, got: {argv:?}"
        );
    }

    #[test]
    fn opencode_surfaces_provider_error_event() {
        let dir = fake_bin(
            "dithus-oc-err",
            "opencode",
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' '{\"type\":\"error\",\"error\":{\"message\":\"Model gone\"}}'\n",
            ),
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            err.0.contains("Model gone"),
            "expected provider message, got: {}",
            err.0
        );
    }

    #[test]
    fn opencode_rejects_nonzero_exit_with_stderr() {
        let dir = fake_bin(
            "dithus-oc-exit",
            "opencode",
            "#!/bin/sh\necho broke >&2\nexit 3\n",
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            err.0.contains("broke"),
            "expected stderr in error, got: {}",
            err.0
        );
    }

    #[test]
    fn opencode_nonzero_exit_keeps_stdout_excerpt() {
        let dir = fake_bin(
            "dithus-oc-exit-out",
            "opencode",
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' '{\"type\":\"error\",\"error\":{\"message\":\"Model gone\"}}'\n",
                "exit 1\n",
            ),
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            err.0.contains("Model gone"),
            "expected stdout excerpt in error, got: {}",
            err.0
        );
    }

    #[test]
    fn opencode_rejects_answerless_output() {
        let dir = fake_bin(
            "dithus-oc-empty",
            "opencode",
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"step_start\"}'\n",
        );
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            err.0.contains("no answer"),
            "expected no-answer error, got: {}",
            err.0
        );
    }

    #[test]
    fn opencode_kills_a_hung_process() {
        let dir = fake_bin("dithus-oc-hung", "opencode", "#!/bin/sh\nsleep 30\n");
        let provider = OpenCodeProvider {
            command: dir.join("opencode").to_str().unwrap().to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(1),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            err.0.contains("timed out"),
            "expected timeout, got: {}",
            err.0
        );
    }

    #[test]
    fn opencode_missing_binary_is_actionable() {
        let provider = OpenCodeProvider {
            command: "/nonexistent/opencode".to_string(),
            model: String::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            err.0.contains("installed"),
            "expected install hint, got: {}",
            err.0
        );
    }

    #[cfg(unix)]
    fn fake_bin(name: &str, file: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("{name}-{}-{}", std::process::id(), unique()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    #[cfg(unix)]
    fn unique() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::SeqCst)
    }
}
