use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_TRANSCRIBER: &str = "whisper-cli";
pub const DEFAULT_RECORDER: &str = "pw-record";

const WAV_HEADER_LEN: u64 = 44;

#[derive(Debug, PartialEq)]
pub struct TranscribeError(pub String);

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, PartialEq)]
pub struct RecordError(pub String);

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn temp_stem(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

pub struct WhisperCli {
    pub command: String,
    pub model: PathBuf,
    pub timeout: Duration,
}

impl WhisperCli {
    pub fn transcribe_wav_file(
        &self,
        wav: &Path,
        language: &str,
    ) -> Result<String, TranscribeError> {
        if !self.model.is_file() {
            return Err(TranscribeError(format!(
                "transcription model missing at {}; download a whisper model \
                 (for example ggml-tiny.bin) from \
                 https://huggingface.co/ggerganov/whisper.cpp and point \
                 transcribe_model at it",
                self.model.display()
            )));
        }
        let stem = temp_stem("dithus-whisper");
        let mut child = std::process::Command::new(&self.command)
            .args([
                "--model",
                self.model.to_str().unwrap(),
                "--file",
                wav.to_str().unwrap(),
                "--language",
                language,
                "--output-txt",
                "--output-file",
                stem.to_str().unwrap(),
                "--no-prints",
            ])
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .map_err(|err| {
                TranscribeError(format!(
                    "whisper-cli failed to start ({}): {err}. \
                     Install whisper.cpp from https://github.com/ggml-org/whisper.cpp \
                     and put whisper-cli on PATH.",
                    self.command
                ))
            })?;
        let mut stderr = child.stderr.take();
        let err_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(ref mut pipe) = stderr {
                use std::io::Read;
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        });
        let status = {
            use wait_timeout::ChildExt;
            match child
                .wait_timeout(self.timeout)
                .map_err(|err| TranscribeError(format!("whisper-cli wait failed: {err}")))?
            {
                Some(status) => status,
                None => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(TranscribeError(format!(
                        "whisper-cli timed out after {}s",
                        self.timeout.as_secs()
                    )));
                }
            }
        };
        let stderr_bytes = err_reader.join().unwrap_or_default();
        if !status.success() {
            return Err(TranscribeError(format!(
                "whisper-cli exited with {status}: {}",
                String::from_utf8_lossy(&stderr_bytes).trim()
            )));
        }
        let text_path = stem.with_extension("txt");
        let raw = std::fs::read_to_string(&text_path)
            .map_err(|err| TranscribeError(format!("cannot read transcript: {err}")))?;
        let _ = std::fs::remove_file(&text_path);
        let transcript = raw.trim().to_string();
        if transcript.is_empty() {
            return Err(TranscribeError(
                "whisper-cli returned no transcript text".to_string(),
            ));
        }
        Ok(transcript)
    }
}

pub fn record_mic_with_command(program: &str, secs: u64, out: &Path) -> Result<(), RecordError> {
    use std::time::{Duration, Instant};

    let mut child = std::process::Command::new(program)
        .args(["--rate", "16000", "--channels", "1"])
        .arg(out)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| {
            RecordError(format!(
                "recorder failed to start ({program}): {err}. \
                 Is PipeWire installed with pw-record on PATH?"
            ))
        })?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match child
            .try_wait()
            .map_err(|err| RecordError(format!("recorder wait failed: {err}")))?
        {
            Some(status) if !status.success() => {
                let mut stderr = String::new();
                if let Some(ref mut pipe) = child.stderr {
                    use std::io::Read;
                    let _ = pipe.read_to_string(&mut stderr);
                }
                return Err(RecordError(format!(
                    "microphone capture failed: {}",
                    stderr.trim()
                )));
            }
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let samples = std::fs::metadata(out).map(|meta| meta.len()).unwrap_or(0);
    if samples <= WAV_HEADER_LEN {
        return Err(RecordError(
            "microphone capture produced no audio; check that a microphone is connected and unmuted"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_mic_audio_to_wav() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = fake_bin(
            "dithus-record-ok",
            "pw-record",
            concat!(
                "#!/bin/sh\n",
                "for a in \"$@\"; do last=\"$a\"; done\n",
                "cp \"tests/fixtures/tone.wav\" \"$last\"\n",
            ),
        );
        let out = dir.join("mic.wav");
        record_mic_with_command(dir.join("pw-record").to_str().unwrap(), 1, &out).unwrap();
        assert_eq!(
            std::fs::read(&out).unwrap(),
            std::fs::read("tests/fixtures/tone.wav").unwrap()
        );
    }

    #[test]
    fn transcribes_fixture_wav_through_binary() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = fake_bin(
            "dithus-whisper-ok",
            "whisper-cli",
            concat!(
                "#!/bin/sh\n",
                "echo \"$@\" > \"$(dirname \"$0\")/argv\"\n",
                "prev=\"\"\n",
                "for a in \"$@\"; do\n",
                "if [ \"$prev\" = \"--file\" ]; then cp \"$a\" \"$(dirname \"$0\")/got.wav\"; fi\n",
                "if [ \"$prev\" = \"--output-file\" ]; then out=\"$a\"; fi\n",
                "prev=\"$a\"\n",
                "done\n",
                "printf 'ola mundo' > \"$out.txt\"\n",
            ),
        );
        let wav = dir.join("in.wav");
        std::fs::write(&wav, [1u8, 2, 3, 4]).unwrap();
        let model = fake_model(&dir);
        let cli = WhisperCli {
            command: dir.join("whisper-cli").to_str().unwrap().to_string(),
            model,
            timeout: Duration::from_secs(5),
        };
        let transcript = cli.transcribe_wav_file(&wav, "pt-BR").unwrap();
        assert_eq!(transcript, "ola mundo");
        let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
        for expected in ["--model", "--file", "--language", "pt-BR", "--output-txt"] {
            assert!(
                argv.contains(expected),
                "expected argv to contain {expected:?}, got: {argv:?}"
            );
        }
        assert_eq!(
            std::fs::read(dir.join("got.wav")).unwrap(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn missing_model_file_names_download_source() {
        let _guard = crate::test_support::write_exec_guard();
        let cli = WhisperCli {
            command: "whisper-cli".to_string(),
            model: PathBuf::from("/nonexistent/ggml-tiny.bin"),
            timeout: Duration::from_secs(5),
        };
        let err = cli
            .transcribe_wav_file(Path::new("tests/fixtures/tone.wav"), "en")
            .unwrap_err();
        assert!(
            err.0.contains("/nonexistent/ggml-tiny.bin"),
            "expected model path, got: {}",
            err.0
        );
        assert!(
            err.0.contains("huggingface.co/ggerganov/whisper.cpp"),
            "expected download source, got: {}",
            err.0
        );
    }

    #[test]
    fn missing_binary_is_actionable() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = std::env::temp_dir().join(format!("dithus-whisper-nobin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = fake_model(&dir);
        let cli = WhisperCli {
            command: "/nonexistent/whisper-cli".to_string(),
            model,
            timeout: Duration::from_secs(5),
        };
        let err = cli
            .transcribe_wav_file(Path::new("tests/fixtures/tone.wav"), "en")
            .unwrap_err();
        assert!(
            err.0.contains("whisper.cpp") && err.0.contains("PATH"),
            "expected install hint, got: {}",
            err.0
        );
    }

    #[test]
    fn nonzero_exit_keeps_stderr() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = fake_bin(
            "dithus-whisper-exit",
            "whisper-cli",
            "#!/bin/sh\necho no-good-model >&2\nexit 3\n",
        );
        let model = fake_model(&dir);
        let cli = WhisperCli {
            command: dir.join("whisper-cli").to_str().unwrap().to_string(),
            model,
            timeout: Duration::from_secs(5),
        };
        let err = cli
            .transcribe_wav_file(Path::new("tests/fixtures/tone.wav"), "en")
            .unwrap_err();
        assert!(
            err.0.contains("no-good-model"),
            "expected stderr in error, got: {}",
            err.0
        );
    }

    #[test]
    fn kills_a_hung_process() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = fake_bin(
            "dithus-whisper-hung",
            "whisper-cli",
            "#!/bin/sh\nsleep 30\n",
        );
        let model = fake_model(&dir);
        let cli = WhisperCli {
            command: dir.join("whisper-cli").to_str().unwrap().to_string(),
            model,
            timeout: Duration::from_secs(1),
        };
        let err = cli
            .transcribe_wav_file(Path::new("tests/fixtures/tone.wav"), "en")
            .unwrap_err();
        assert!(
            err.0.contains("timed out"),
            "expected timeout, got: {}",
            err.0
        );
    }

    #[test]
    fn empty_transcript_is_rejected() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = fake_bin(
            "dithus-whisper-empty",
            "whisper-cli",
            concat!(
                "#!/bin/sh\n",
                "prev=\"\"\n",
                "for a in \"$@\"; do\n",
                "if [ \"$prev\" = \"--output-file\" ]; then out=\"$a\"; fi\n",
                "prev=\"$a\"\n",
                "done\n",
                "printf '   ' > \"$out.txt\"\n",
            ),
        );
        let model = fake_model(&dir);
        let cli = WhisperCli {
            command: dir.join("whisper-cli").to_str().unwrap().to_string(),
            model,
            timeout: Duration::from_secs(5),
        };
        let err = cli
            .transcribe_wav_file(Path::new("tests/fixtures/tone.wav"), "en")
            .unwrap_err();
        assert!(
            err.0.contains("no transcript"),
            "expected no-transcript error, got: {}",
            err.0
        );
    }

    #[test]
    fn recorder_failure_names_the_device_problem() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = fake_bin(
            "dithus-record-fail",
            "pw-record",
            "#!/bin/sh\necho no such device >&2\nexit 2\n",
        );
        let out = dir.join("mic.wav");
        let err =
            record_mic_with_command(dir.join("pw-record").to_str().unwrap(), 1, &out).unwrap_err();
        assert!(
            err.0.contains("no such device"),
            "expected device error, got: {err}"
        );
    }

    #[test]
    fn silent_recorder_reports_no_audio() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = fake_bin("dithus-record-silent", "pw-record", "#!/bin/sh\nexit 0\n");
        let out = dir.join("mic.wav");
        let err =
            record_mic_with_command(dir.join("pw-record").to_str().unwrap(), 1, &out).unwrap_err();
        assert!(
            err.0.contains("no audio"),
            "expected no-audio hint, got: {err}"
        );
    }

    #[test]
    fn truncated_wav_reports_no_audio() {
        let _guard = crate::test_support::write_exec_guard();
        let dir = fake_bin(
            "dithus-record-truncated",
            "pw-record",
            "#!/bin/sh\nfor a in \"$@\"; do last=\"$a\"; done\nhead -c 10 /dev/zero > \"$last\"\n",
        );
        let out = dir.join("mic.wav");
        let err =
            record_mic_with_command(dir.join("pw-record").to_str().unwrap(), 1, &out).unwrap_err();
        assert!(
            err.0.contains("no audio"),
            "expected no-audio hint, got: {err}"
        );
    }

    fn fake_model(dir: &std::path::Path) -> std::path::PathBuf {
        let model = dir.join("ggml-tiny.bin");
        std::fs::write(&model, b"fakemodel").unwrap();
        model
    }

    #[cfg(unix)]
    fn fake_bin(name: &str, file: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }
}
