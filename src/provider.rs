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

pub struct OllamaProvider {
    pub base_url: String,
    pub model: String,
    pub timeout: std::time::Duration,
}

pub struct OpenAIProvider {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub timeout: std::time::Duration,
}

impl ChatProvider for OpenAIProvider {
    fn ask(&self, question: &str, image: &[u8]) -> Result<String, ProviderError> {
        let body = chat_request_body(&self.model, question, image);
        post_chat(&self.base_url, Some(&self.api_key), &body, self.timeout)
    }
}

impl ChatProvider for OllamaProvider {
    fn ask(&self, question: &str, image: &[u8]) -> Result<String, ProviderError> {
        let body = chat_request_body(&self.model, question, image);
        post_chat(&self.base_url, None, &body, self.timeout).map_err(|err| {
            ProviderError(format!(
                "{}Is Ollama running at {}?",
                err.0,
                host_of(&self.base_url)
            ))
        })
    }
}

fn chat_request_body(model: &str, question: &str, image: &[u8]) -> serde_json::Value {
    let mut content = vec![serde_json::json!({"type": "text", "text": question})];
    if !image.is_empty() {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": format!("data:image/png;base64,{}", base64_encode(image))},
        }));
    }
    serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": content}],
    })
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn host_of(base_url: &str) -> &str {
    base_url
        .split("://")
        .nth(1)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or(base_url)
}

fn post_chat(
    base_url: &str,
    api_key: Option<&str>,
    body: &serde_json::Value,
    timeout: std::time::Duration,
) -> Result<String, ProviderError> {
    use ureq::Agent;

    let config = Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();
    let agent = Agent::new_with_config(config);
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut request = agent.post(&url);
    if let Some(key) = api_key {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    let send_error = |err: ureq::Error| {
        ProviderError(match &err {
            ureq::Error::Timeout(_) => {
                format!("request to {url} timed out after {}s. ", timeout.as_secs())
            }
            ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
                format!("cannot reach {url} ({err}). ")
            }
            _ => format!("request to {url} failed ({err}). "),
        })
    };
    let mut response = request.send_json(body).map_err(send_error)?;
    let status = response.status();
    if !(200..300).contains(&status.as_u16()) {
        let excerpt: String = response
            .body_mut()
            .read_to_string()
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect();
        return Err(ProviderError(format!(
            "request to {url} failed with status {status}. {excerpt} "
        )));
    }
    let json: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|err| ProviderError(format!("invalid JSON from {url}: {err}. ")))?;
    json.get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(String::from)
        .filter(|answer| !answer.is_empty())
        .ok_or_else(|| ProviderError(format!("response from {url} held no answer text. ")))
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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
        let _guard = crate::test_support::write_exec_guard();
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

    #[test]
    fn ollama_posts_chat_request_and_returns_answer() {
        let (base_url, request) = serve_once(
            200,
            r#"{"choices":[{"message":{"role":"assistant","content":"local answer"}}]}"#,
        );
        let provider = OllamaProvider {
            base_url,
            model: "test-model".to_string(),
            timeout: std::time::Duration::from_secs(5),
        };
        let answer = provider.ask("what is on my screen?", &[1, 2]).unwrap();
        assert_eq!(answer, "local answer");
        let raw = request.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        for expected in [
            "POST /chat/completions",
            "\"model\"",
            "\"test-model\"",
            "what is on my screen?",
            "data:image/png;base64,AQI=",
        ] {
            assert!(
                raw.contains(expected),
                "expected request to contain {expected:?}, got: {raw:?}"
            );
        }
    }

    #[test]
    fn ollama_unreachable_names_host_and_port() {
        let provider = OllamaProvider {
            base_url: "http://127.0.0.1:9".to_string(),
            model: "test-model".to_string(),
            timeout: std::time::Duration::from_secs(5),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            err.0.contains("127.0.0.1:9") && err.0.contains("running"),
            "expected host hint, got: {}",
            err.0
        );
    }

    #[test]
    fn ollama_status_error_keeps_server_reason() {
        let (base_url, _request) = serve_once(
            404,
            r#"{"error":"model 'nope' not found, try pulling it first"}"#,
        );
        let provider = OllamaProvider {
            base_url,
            model: "nope".to_string(),
            timeout: std::time::Duration::from_secs(5),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            err.0.contains("404") && err.0.contains("not found"),
            "expected status and reason, got: {}",
            err.0
        );
    }

    #[test]
    fn openai_sends_bearer_key_and_returns_answer() {
        let (base_url, request) = serve_once(
            200,
            r#"{"choices":[{"message":{"role":"assistant","content":"cloud answer"}}]}"#,
        );
        let provider = OpenAIProvider {
            base_url,
            model: "test-model".to_string(),
            api_key: "test-key-abc".to_string(),
            timeout: std::time::Duration::from_secs(5),
        };
        let answer = provider.ask("what is on my screen?", &[1, 2]).unwrap();
        assert_eq!(answer, "cloud answer");
        let raw = request.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        for expected in [
            "POST /chat/completions",
            "authorization: Bearer test-key-abc",
            "\"test-model\"",
            "what is on my screen?",
            "data:image/png;base64,AQI=",
        ] {
            assert!(
                raw.contains(expected),
                "expected request to contain {expected:?}, got: {raw:?}"
            );
        }
    }

    #[test]
    fn openai_rejection_never_leaks_key() {
        let (base_url, _request) = serve_once(
            401,
            r#"{"error":{"message":"bad key","type":"invalid_request_error"}}"#,
        );
        let provider = OpenAIProvider {
            base_url,
            model: "test-model".to_string(),
            api_key: "test-key-abc".to_string(),
            timeout: std::time::Duration::from_secs(5),
        };
        let err = provider.ask("hi", &[]).unwrap_err();
        assert!(
            !err.0.contains("test-key-abc"),
            "key leaked into error: {}",
            err.0
        );
        assert!(
            err.0.contains("401"),
            "expected status in error, got: {}",
            err.0
        );
    }

    fn serve_once(status: u16, body: &str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_string();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let header_end = loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break None;
                }
                raw.extend_from_slice(&buf[..n]);
                if let Some(pos) = find_header_end(&raw) {
                    break Some(pos);
                }
            };
            if let Some(end) = header_end {
                if String::from_utf8_lossy(&raw).contains("Expect: 100-continue") {
                    stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").unwrap();
                }
                let content_length = content_length_of(&raw).unwrap_or(0);
                while raw.len() < end + content_length {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                }
            }
            sender.send(String::from_utf8_lossy(&raw).into_owned()).unwrap();
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
                reason = if status == 200 { "OK" } else { "Error" },
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (base_url, receiver)
    }

    fn find_header_end(raw: &[u8]) -> Option<usize> {
        raw.windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|pos| pos + 4)
    }

    fn content_length_of(raw: &[u8]) -> Option<usize> {
        String::from_utf8_lossy(raw)
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("content-length") {
                    value.trim().parse().ok()
                } else {
                    None
                }
            })
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
