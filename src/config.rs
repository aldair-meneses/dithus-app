use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_ollama_model")]
    pub ollama_model: String,
    #[serde(default = "default_openai_model")]
    pub openai_model: String,
    #[serde(default = "default_opencode_model")]
    pub opencode_model: String,
    #[serde(default = "default_capture_interval_secs")]
    pub capture_interval_secs: u64,
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_provider() -> String {
    "stub".to_string()
}

fn default_ollama_model() -> String {
    "qwen2.5vl".to_string()
}

fn default_openai_model() -> String {
    "gpt-4o-mini".to_string()
}

fn default_opencode_model() -> String {
    String::new()
}

fn default_capture_interval_secs() -> u64 {
    5
}

fn default_language() -> String {
    "en".to_string()
}

impl Default for Config {
    fn default() -> Self {
        toml::from_str("").expect("empty TOML must yield defaults")
    }
}

pub fn load_config(path: &Path) -> Config {
    let content = std::fs::read_to_string(path).unwrap_or_else(|_| {
        eprintln!("config not found at {}, using defaults", path.display());
        String::new()
    });
    toml::from_str(&content).expect("config file must be valid TOML")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn missing_file_yields_defaults() {
        let config = load_config(Path::new("/nonexistent/dithus.toml"));
        assert_eq!(config, Config::default());
    }

    #[test]
    fn full_toml_parses_to_expected_config() {
        let mut file = tempfile_path("dithus-full");
        writeln!(
            file,
            concat!(
                "provider = \"ollama\"\n",
                "ollama_model = \"llama3.2-vision\"\n",
                "openai_model = \"gpt-4o\"\n",
                "opencode_model = \"pro\"\n",
                "capture_interval_secs = 10\n",
                "language = \"pt-BR\"\n",
            )
        )
        .unwrap();

        let config = load_config(&test_temp_path("dithus-full"));
        assert_eq!(
            config,
            Config {
                provider: "ollama".to_string(),
                ollama_model: "llama3.2-vision".to_string(),
                openai_model: "gpt-4o".to_string(),
                opencode_model: "pro".to_string(),
                capture_interval_secs: 10,
                language: "pt-BR".to_string(),
            }
        );
    }

    #[test]
    fn partial_toml_fills_absent_keys_with_defaults() {
        let mut file = tempfile_path("dithus-partial");
        writeln!(file, "provider = \"openai\"\n").unwrap();

        let config = load_config(&test_temp_path("dithus-partial"));
        assert_eq!(config.provider, "openai");
        assert_eq!(config, {
            let mut expected = Config::default();
            expected.provider = "openai".to_string();
            expected
        });
    }

    fn tempfile_path(name: &str) -> std::fs::File {
        std::fs::File::create(test_temp_path(name)).unwrap()
    }

    fn test_temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("{name}-{}", std::process::id()))
    }
}
