use std::time::Duration;

#[derive(Debug, PartialEq)]
pub enum Backend {
    Portal,
    Grim,
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Portal => "portal",
            Backend::Grim => "grim",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct CapturedScreen {
    pub png: Vec<u8>,
    pub backend: Backend,
}

#[derive(Debug, PartialEq)]
pub struct CaptureError(pub String);

pub fn grim_shot_with_command(program: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use wait_timeout::ChildExt;

    let mut child = Command::new(program)
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("grim failed to start ({program}): {err}"))?;

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
        .map_err(|err| format!("grim wait failed: {err}"))?
    {
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!(
                "grim timed out after {}s; check that it can see WAYLAND_DISPLAY",
                timeout.as_secs()
            ))
        }
        Some(status) => {
            let stdout_bytes = out_reader.join().unwrap_or_default();
            let stderr_bytes = err_reader.join().unwrap_or_default();
            if !status.success() {
                return Err(format!(
                    "grim exited with {status}: {}",
                    String::from_utf8_lossy(&stderr_bytes).trim()
                ));
            }
            if stdout_bytes.is_empty() {
                return Err("grim produced no output".to_string());
            }
            Ok(stdout_bytes)
        }
    }
}

pub fn screenshot_via_portal() -> Result<Vec<u8>, String> {
    screenshot_via_portal_with_timeout(Duration::from_secs(25))
}

pub fn screenshot_via_portal_with_timeout(timeout: Duration) -> Result<Vec<u8>, String> {
    use dbus::arg::{RefArg, Variant};
    use dbus::blocking::Connection;
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::time::Instant;

    let conn =
        Connection::new_session().map_err(|err| format!("cannot reach session bus: {err}"))?;
    let token = format!("dithus{}", std::process::id());
    let expected_path = portal_request_path(&conn.unique_name().to_string(), &token);

    let (sender, receiver) = mpsc::channel();
    let rule = dbus::message::MatchRule::new_signal("org.freedesktop.portal.Request", "Response");
    conn.add_match(
        rule,
        move |(response, results): (u32, HashMap<String, Variant<Box<dyn RefArg>>>),
              _conn: &Connection,
              msg: &dbus::message::Message| {
            if msg.path().map(|path| path.to_string()) == Some(expected_path.clone()) {
                let uri = results
                    .get("uri")
                    .and_then(|variant| variant.0.as_str())
                    .map(String::from);
                let _ = sender.send((response, uri));
            }
            true
        },
    )
    .map_err(|err| format!("cannot subscribe to portal response: {err}"))?;

    let proxy = conn.with_proxy(
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        Duration::from_secs(10),
    );
    let mut options: HashMap<&str, Variant<Box<dyn RefArg>>> = HashMap::new();
    options.insert(
        "handle_token",
        Variant(Box::new(token) as Box<dyn RefArg>),
    );
    options.insert("modal", Variant(Box::new(true) as Box<dyn RefArg>));
    options.insert(
        "interactive",
        Variant(Box::new(false) as Box<dyn RefArg>),
    );
    let (_request,): (dbus::Path,) = proxy
        .method_call(
            "org.freedesktop.portal.Screenshot",
            "Screenshot",
            ("", options),
        )
        .map_err(|err| format!("portal Screenshot call failed: {err}"))?;

    let deadline = Instant::now() + timeout;
    loop {
        if let Ok((response, uri)) = receiver.try_recv() {
            if response != 0 {
                return Err(format!("portal Screenshot refused the request (code {response})"));
            }
            let uri = uri.ok_or("portal Screenshot answered without an image URI")?;
            let path = uri_to_path(&uri)?;
            return std::fs::read(&path)
                .map_err(|err| format!("cannot read portal screenshot {}: {err}", path.display()));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "portal Screenshot timed out after {}s",
                timeout.as_secs()
            ));
        }
        conn.process(Duration::from_millis(100))
            .map_err(|err| format!("D-Bus pump failed: {err}"))?;
    }
}

pub fn capture_screenshot() -> Result<CapturedScreen, CaptureError> {
    capture_with(screenshot_via_portal, screenshot_via_grim_bytes)
}

fn screenshot_via_grim_bytes() -> Result<Vec<u8>, String> {
    grim_shot_with_command("grim", Duration::from_secs(15))
}

pub fn portal_request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name
        .strip_prefix(':')
        .unwrap_or(unique_name)
        .replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

pub fn uri_to_path(uri: &str) -> Result<std::path::PathBuf, String> {
    uri.strip_prefix("file://")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| format!("portal returned a non-file URI: {uri}"))
}

pub fn capture_with(
    try_portal: impl FnOnce() -> Result<Vec<u8>, String>,
    try_grim: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<CapturedScreen, CaptureError> {
    match try_portal() {
        Ok(png) => Ok(CapturedScreen {
            png,
            backend: Backend::Portal,
        }),
        Err(portal_err) => {
            eprintln!("portal screenshot failed ({portal_err}), trying grim");
            match try_grim() {
                Ok(png) => Ok(CapturedScreen {
                    png,
                    backend: Backend::Grim,
                }),
                Err(grim_err) => Err(CaptureError(format!(
                    "screen capture failed (portal: {portal_err}; grim: {grim_err}). \
                     On Sway, check that xdg-desktop-portal is running or that grim can see WAYLAND_DISPLAY."
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_success_wins_with_portal_backend() {
        let screen = capture_with(|| Ok(vec![1, 2, 3]), || Ok(vec![9])).unwrap();
        assert_eq!(
            screen,
            CapturedScreen {
                png: vec![1, 2, 3],
                backend: Backend::Portal,
            }
        );
    }

    #[test]
    fn portal_failure_falls_through_to_grim() {
        let screen = capture_with(|| Err("portal down".to_string()), || Ok(vec![7, 8])).unwrap();
        assert_eq!(
            screen,
            CapturedScreen {
                png: vec![7, 8],
                backend: Backend::Grim,
            }
        );
    }

    #[test]
    fn both_failing_names_the_remedy() {
        let err = capture_with(
            || Err("no portal".to_string()),
            || Err("no wayland display".to_string()),
        )
        .unwrap_err();
        assert_eq!(
            err,
            CaptureError(
                "screen capture failed (portal: no portal; grim: no wayland display). \
                 On Sway, check that xdg-desktop-portal is running or that grim can see WAYLAND_DISPLAY."
                    .to_string()
            )
        );
    }

    #[test]
    fn grim_adapter_returns_program_stdout() {
        let _guard = crate::test_support::write_exec_guard();
        let program = fake_script("dithus-grim-ok", "#!/bin/sh\nprintf 'fakepng'");
        let png = grim_shot_with_command(&program, Duration::from_secs(5)).unwrap();
        assert_eq!(png, b"fakepng".to_vec());
    }

    #[test]
    fn grim_adapter_reports_failing_exit_with_stderr() {
        let _guard = crate::test_support::write_exec_guard();
        let program = fake_script("dithus-grim-fail", "#!/bin/sh\necho boom >&2\nexit 1");
        let err = grim_shot_with_command(&program, Duration::from_secs(5)).unwrap_err();
        assert!(
            err.contains("boom"),
            "expected grim stderr in error, got: {err}"
        );
    }

    #[test]
    fn grim_adapter_kills_a_hung_program() {
        let _guard = crate::test_support::write_exec_guard();
        let program = fake_script("dithus-grim-hung", "#!/bin/sh\nsleep 30");
        let err = grim_shot_with_command(&program, Duration::from_secs(1)).unwrap_err();
        assert!(
            err.contains("timed out"),
            "expected timeout in error, got: {err}"
        );
    }

    #[test]
    fn grim_adapter_drains_large_stdout() {
        let _guard = crate::test_support::write_exec_guard();
        let program = fake_script("dithus-grim-big", "#!/bin/sh\nhead -c 200000 /dev/zero");
        let png = grim_shot_with_command(&program, Duration::from_secs(10)).unwrap();
        assert_eq!(png, vec![0u8; 200000]);
    }

    #[test]
    fn portal_request_path_follows_portal_naming() {
        assert_eq!(
            portal_request_path(":1.247", "dithus42"),
            "/org/freedesktop/portal/desktop/request/1_247/dithus42"
        );
    }

    #[test]
    fn file_uri_maps_to_filesystem_path() {
        assert_eq!(
            uri_to_path("file:///tmp/shot.png").unwrap(),
            std::path::PathBuf::from("/tmp/shot.png")
        );
    }

    #[test]
    fn non_file_uri_is_rejected() {
        assert!(uri_to_path("https:///tmp/shot.png").is_err());
    }

    #[cfg(unix)]
    fn fake_script(name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_string()
    }
}
