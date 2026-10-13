use std::process::Command;

fn dithus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dithus"))
}

fn reliable_output(cmd: &mut Command) -> std::process::Output {
    let mut output = cmd.output().unwrap();
    for _ in 0..3 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() || !stderr.contains("Text file busy") {
            break;
        }
        output = cmd.output().unwrap();
    }
    output
}

#[test]
fn ask_with_fixture_image_prints_stub_answer() {
    let output = dithus()
        .args([
            "ask",
            "--question",
            "what is on my screen?",
            "--image",
            "tests/fixtures/screen.png",
            "--config",
            "/nonexistent/dithus.toml",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "stub answer: what is on my screen?\n"
    );
}

#[test]
fn missing_config_falls_back_to_defaults_with_log() {
    let output = dithus()
        .args([
            "ask",
            "--question",
            "hi",
            "--image",
            "tests/fixtures/screen.png",
            "--config",
            "/nonexistent/dithus.toml",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("using defaults"),
        "expected defaults notice on stderr, got: {stderr}"
    );
}

#[test]
fn unconfigured_provider_fails_with_clear_message() {
    let dir = std::env::temp_dir().join(format!("dithus-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("dithus.toml");
    std::fs::write(&config_path, "provider = \"pluely\"\n").unwrap();

    let output = dithus()
        .args([
            "ask",
            "--question",
            "hi",
            "--image",
            "tests/fixtures/screen.png",
            "--config",
            config_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("pluely") && stderr.contains("not yet implemented"),
        "expected clear unimplemented-provider message, got: {stderr}"
    );
}

fn write_exec(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn whisper_cli_stub(transcript: &str, log_argv: bool) -> String {
    format!(
        concat!(
            "#!/bin/sh\n",
            "{argv}",
            "prev=\"\"\n",
            "for a in \"$@\"; do\n",
            "if [ \"$prev\" = \"--output-file\" ]; then out=\"$a\"; fi\n",
            "prev=\"$a\"\n",
            "done\n",
            "printf '{transcript}' > \"$out.txt\"\n",
        ),
        argv = if log_argv {
            "echo \"$@\" > \"$(dirname \"$0\")/argv\"\n"
        } else {
            ""
        },
        transcript = transcript,
    )
}

fn serve_chat_once(answer: &str) -> (String, std::sync::mpsc::Receiver<String>) {

    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let body = format!(
        r#"{{"choices":[{{"message":{{"role":"assistant","content":{answer:?}}}}}]}}"#,
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
            if let Some(end) = raw
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|pos| pos + 4)
            {
                let length = String::from_utf8_lossy(&raw)
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.trim().eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                while raw.len() < end + length {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                }
                break;
            }
        }
        sender.send(String::from_utf8_lossy(&raw).into_owned()).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    (base_url, receiver)
}

#[test]
fn ask_with_ollama_provider_hits_configured_base_url() {
    let (base_url, request) = serve_chat_once("mock answer");
    let dir = std::env::temp_dir().join(format!("dithus-ollama-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("dithus.toml"),
        "provider = \"ollama\"\nollama_model = \"probe-model\"\n",
    )
    .unwrap();

    let output = dithus()
        .env("OLLAMA_BASE_URL", &base_url)
        .args([
            "ask",
            "--question",
            "what is on my screen?",
            "--image",
            "tests/fixtures/screen.png",
            "--config",
            dir.join("dithus.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "mock answer\n"
    );
    let raw = request.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(
        raw.contains("POST /chat/completions") && raw.contains("probe-model"),
        "expected chat request at mock server, got: {raw:?}"
    );
}

#[test]
fn ask_with_openai_provider_uses_env_key() {
    let (base_url, request) = serve_chat_once("cloud answer");
    let dir = std::env::temp_dir().join(format!("dithus-openai-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("dithus.toml"),
        "provider = \"openai\"\nopenai_model = \"probe-model\"\n",
    )
    .unwrap();

    let output = dithus()
        .env("OPENAI_BASE_URL", &base_url)
        .env("OPENAI_API_KEY", "test-key-abc")
        .args([
            "ask",
            "--question",
            "what is on my screen?",
            "--image",
            "tests/fixtures/screen.png",
            "--config",
            dir.join("dithus.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "cloud answer\n"
    );
    let raw = request.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(
        raw.contains("authorization: Bearer test-key-abc"),
        "expected bearer key at mock server, got: {raw:?}"
    );
}

#[test]
fn openai_missing_key_names_the_variable() {
    let dir = std::env::temp_dir().join(format!("dithus-nokey-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dithus.toml"), "provider = \"openai\"\n").unwrap();

    let output = dithus()
        .env_remove("OPENAI_API_KEY")
        .args([
            "ask",
            "--question",
            "hi",
            "--image",
            "tests/fixtures/screen.png",
            "--config",
            dir.join("dithus.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("OPENAI_API_KEY"),
        "expected variable name in error, got: {stderr:?}"
    );
    assert!(
        !stderr.contains("test-key-abc"),
        "unexpected key material in error: {stderr:?}"
    );
}

#[test]
fn ask_without_image_runs_full_chain_to_mock_provider() {
    let dir = std::env::temp_dir().join(format!("dithus-chain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("grim-ran");
    let fixture = std::path::absolute("tests/fixtures/screen.png").unwrap();
    std::fs::write(
        dir.join("grim"),
        format!(
            "#!/bin/sh\ntouch {}\ncat {}\n",
            marker.display(),
            fixture.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("grim"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    let (base_url, _request) = serve_chat_once("chained answer");
    std::fs::write(dir.join("dithus.toml"), "provider = \"ollama\"\n").unwrap();
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = reliable_output(
        dithus()
            .env("PATH", path)
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
            .env("OLLAMA_BASE_URL", &base_url)
            .args([
                "ask",
                "--question",
                "what is on my screen?",
                "--config",
                dir.join("dithus.toml").to_str().unwrap(),
            ]),
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "chained answer\n"
    );
    assert!(
        marker.exists(),
        "expected screen capture to run in the chain"
    );
}

#[test]
fn ask_logs_provider_and_model_stage() {
    let (base_url, _request) = serve_chat_once("mock answer");
    let dir = std::env::temp_dir().join(format!("dithus-stage-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("dithus.toml"),
        "provider = \"ollama\"\nollama_model = \"probe-model\"\n",
    )
    .unwrap();

    let output = dithus()
        .env("OLLAMA_BASE_URL", &base_url)
        .args([
            "ask",
            "--question",
            "hi",
            "--image",
            "tests/fixtures/screen.png",
            "--config",
            dir.join("dithus.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("ollama") && stderr.contains("probe-model"),
        "expected provider stage log, got: {stderr:?}"
    );
}

#[test]
fn ask_repeat_prints_an_answer_per_interval() {
    use std::io::{BufRead, BufReader};

    let mut child = dithus()
        .args([
            "ask",
            "--question",
            "repeat q",
            "--image",
            "tests/fixtures/screen.png",
            "--config",
            "/nonexistent/dithus.toml",
            "--repeat",
            "--interval",
            "0",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut answers = Vec::new();
    for _ in 0..2 {
        answers.push(
            receiver
                .recv_timeout(std::time::Duration::from_secs(15))
                .unwrap(),
        );
    }
    child.kill().unwrap();
    let _ = child.wait();
    assert_eq!(answers, vec!["stub answer: repeat q".to_string(); 2]);
}

#[test]
fn ask_with_opencode_provider_uses_configured_model() {
    let dir = std::env::temp_dir().join(format!("dithus-oc-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("opencode"),
        concat!(
            "#!/bin/sh\n",
            "echo \"$@\" > \"$(dirname \"$0\")/argv\"\n",
            "printf '%s\\n' '{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"live answer\"}}'\n",
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("opencode"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    std::fs::write(
        dir.join("dithus.toml"),
        "provider = \"opencode\"\nopencode_model = \"probe-model\"\n",
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = reliable_output(
        dithus()
            .env("PATH", path)
            .args([
                "ask",
                "--question",
                "what is on my screen?",
                "--image",
                "tests/fixtures/screen.png",
                "--config",
                dir.join("dithus.toml").to_str().unwrap(),
            ]),
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "live answer\n"
    );
    let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
    assert!(
        argv.contains("--model probe-model"),
        "expected configured model in argv, got: {argv:?}"
    );
}

#[test]
fn listen_with_fixture_audio_prints_transcript() {
    let dir = std::env::temp_dir().join(format!("dithus-listen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_exec(
        &dir.join("whisper-cli"),
        &whisper_cli_stub("ola mundo", true),
    );
    let model = dir.join("ggml-tiny.bin");
    std::fs::write(&model, b"fakemodel").unwrap();
    std::fs::write(
        dir.join("dithus.toml"),
        format!(
            "transcribe_model = \"{}\"\nlanguage = \"pt-BR\"\n",
            model.display()
        ),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = reliable_output(dithus().env("PATH", path).args([
        "listen",
        "--audio",
        "tests/fixtures/tone.wav",
        "--config",
        dir.join("dithus.toml").to_str().unwrap(),
    ]));
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "ola mundo\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("pt-BR"),
        "expected language stage log, got: {stderr:?}"
    );
    let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
    assert!(
        argv.contains("--language pt-BR"),
        "expected language in argv, got: {argv:?}"
    );
}

#[test]
fn listen_without_audio_records_mic_then_transcribes() {
    let dir = std::env::temp_dir().join(format!("dithus-listen-mic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = std::path::absolute("tests/fixtures/tone.wav").unwrap();
    let marker = dir.join("mic-ran");
    write_exec(
        &dir.join("pw-record"),
        &format!(
            "#!/bin/sh\ntouch {}\nfor a in \"$@\"; do last=\"$a\"; done\ncat {} > \"$last\"\n",
            marker.display(),
            fixture.display()
        ),
    );
    write_exec(
        &dir.join("whisper-cli"),
        &whisper_cli_stub("chained transcript", false),
    );
    let model = dir.join("ggml-tiny.bin");
    std::fs::write(&model, b"fakemodel").unwrap();
    std::fs::write(
        dir.join("dithus.toml"),
        format!("transcribe_model = \"{}\"\n", model.display()),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = reliable_output(dithus().env("PATH", path).args([
        "listen",
        "--secs",
        "1",
        "--config",
        dir.join("dithus.toml").to_str().unwrap(),
    ]));
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "chained transcript\n"
    );
    assert!(marker.exists(), "expected the recorder to run in the chain");
}

#[test]
fn listen_missing_model_logs_plainly() {
    let dir = std::env::temp_dir().join(format!("dithus-listen-nomodel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("dithus.toml"),
        "transcribe_model = \"/nonexistent/ggml-x.bin\"\n",
    )
    .unwrap();

    let output = dithus()
        .args([
            "listen",
            "--audio",
            "tests/fixtures/tone.wav",
            "--config",
            dir.join("dithus.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("/nonexistent/ggml-x.bin") && stderr.contains("huggingface.co"),
        "expected model path and source, got: {stderr:?}"
    );
}

#[test]
fn ask_without_image_captures_screen_and_names_backend() {
    let dir = std::env::temp_dir().join(format!("dithus-grim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("grim-ran");
    let fixture = std::path::absolute("tests/fixtures/screen.png").unwrap();
    std::fs::write(
        dir.join("grim"),
        format!(
            "#!/bin/sh\ntouch {}\ncat {}\n",
            marker.display(),
            fixture.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.join("grim"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = reliable_output(
        dithus()
            .env("PATH", path)
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
            .args([
                "ask",
                "--question",
                "what is on my screen?",
                "--config",
                "/nonexistent/dithus.toml",
            ]),
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "stub answer: what is on my screen?\n"
    );
    assert!(
        marker.exists(),
        "expected the grim backend to run during capture"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("grim"),
        "expected backend name on stderr, got: {stderr}"
    );
}
