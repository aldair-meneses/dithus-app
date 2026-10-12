use std::process::Command;

fn dithus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dithus"))
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
    std::fs::write(&config_path, "provider = \"ollama\"\n").unwrap();

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
        stderr.contains("ollama") && stderr.contains("not yet implemented"),
        "expected clear unimplemented-provider message, got: {stderr}"
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

    let output = dithus()
        .env("PATH", path)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
        .args([
            "ask",
            "--question",
            "what is on my screen?",
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
