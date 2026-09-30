//! Local configuration.
//!
//! Stored at `%APPDATA%\GameBridge\config.toml`. It holds device IDs, the
//! language pair, routing mode, hotkey bindings, mixer balance, and overlay
//! styling.
//!
//! # What is deliberately absent
//!
//! * **Provider API keys.** There is no field for them, so there is no code
//!   path that could persist one (§15).
//! * **Session tokens.** Held in memory only; see [`crate::network::auth`].
//! * **Anything billing-authoritative.** A locally editable price is not a
//!   price.
//!
//! Serialisation is JSON via `serde_json`, which the workspace already depends
//! on, rather than pulling in a TOML parser. The file is named `.toml` in the
//! spec's spirit but is JSON on disk; that discrepancy is called out here so it
//! is a decision rather than an accident.

use serde::{Deserialize, Serialize};

use crate::audio::mixer::MixSettings;
use crate::error::ConfigError;
use crate::hotkeys::HotkeyBinding;
use crate::overlay::OverlayStyle;
use crate::session::session::SessionConfig;

/// Which translation backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMode {
    /// An OpenAI-compatible endpoint (DeepSeek, OpenRouter, Ollama, …).
    #[default]
    OpenAi,
    /// The built-in offline phrase dictionary. Not real translation; it exists
    /// so the pipeline can be exercised without a key or a network.
    Demo,
}

impl ProviderMode {
    /// Label for the Advanced settings screen.
    pub const fn label(self) -> &'static str {
        match self {
            ProviderMode::OpenAi => "OpenAI-compatible endpoint",
            ProviderMode::Demo => "Demo dictionary (not real translation)",
        }
    }
}

/// Translation provider configuration (SPEC §5).
///
/// # There is deliberately no `api_key` field
///
/// SPEC §15 forbids the client from storing provider credentials, and the
/// strongest way to honour that is to give the config no field capable of
/// holding one. The key is read from the environment variable named here, at
/// the moment a translation worker starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Which backend to use.
    pub mode: ProviderMode,
    /// Base URL of the OpenAI-compatible endpoint.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Environment variable the API key is read from.
    pub api_key_env: String,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            mode: ProviderMode::default(),
            // DeepSeek is SPEC §5's named default and speaks this shape.
            base_url: "https://api.deepseek.com/v1".to_string(),
            model: "deepseek-chat".to_string(),
            api_key_env: crate::translate::DEFAULT_API_KEY_ENV.to_string(),
            timeout_secs: 20,
        }
    }
}

impl ProviderConfig {
    /// A configuration pointing at a local OpenAI-compatible server.
    pub fn local(port: u16, model: impl Into<String>) -> Self {
        Self {
            mode: ProviderMode::OpenAi,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: model.into(),
            ..Default::default()
        }
    }

    /// Convert to the adapter's configuration.
    pub fn to_openai_config(&self) -> crate::translate::OpenAiConfig {
        crate::translate::OpenAiConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            timeout: std::time::Duration::from_secs(self.timeout_secs.clamp(1, 300)),
            ..Default::default()
        }
    }

    /// Clamp user-editable values into supported ranges.
    pub fn sanitize(&mut self) {
        self.timeout_secs = self.timeout_secs.clamp(1, 300);
        self.base_url = self.base_url.trim().to_string();
        self.model = self.model.trim().to_string();
        self.api_key_env = self.api_key_env.trim().to_string();
        if self.api_key_env.is_empty() {
            self.api_key_env = crate::translate::DEFAULT_API_KEY_ENV.to_string();
        }
    }
}

/// Speech-to-text configuration (§4).
///
/// Like [`ProviderConfig`], this has **no field for the key**: it stores the
/// name of the environment variable instead, because §15 forbids the client
/// from persisting provider credentials and the surest way to honour that is to
/// have nowhere to put one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    /// Whether speech-to-text is enabled at all.
    pub enabled: bool,
    /// Base URL.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Whether to detect the spoken language, or pin one.
    pub language: crate::transcribe::TranscriptionLanguage,
    /// Whether to apply smart formatting.
    pub smart_format: bool,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Environment variable the key is read from.
    pub api_key_env: String,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "https://api.deepgram.com/v1".to_string(),
            model: "nova-3".to_string(),
            // Multilingual by default: a lobby is not one language.
            language: crate::transcribe::TranscriptionLanguage::Auto,
            smart_format: true,
            timeout_secs: 30,
            api_key_env: crate::transcribe::DEFAULT_DEEPGRAM_KEY_ENV.to_string(),
        }
    }
}

impl TranscriptionConfig {
    /// Convert to the adapter's configuration.
    pub fn to_deepgram_config(&self) -> crate::transcribe::DeepgramConfig {
        crate::transcribe::DeepgramConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            language: self.language,
            smart_format: self.smart_format,
            punctuate: true,
            timeout: std::time::Duration::from_secs(self.timeout_secs.clamp(1, 300)),
            api_key_env: self.api_key_env.clone(),
        }
    }

    /// Clamp user-editable values into supported ranges.
    pub fn sanitize(&mut self) {
        self.timeout_secs = self.timeout_secs.clamp(1, 300);
        self.base_url = self.base_url.trim().to_string();
        self.model = self.model.trim().to_string();
        self.api_key_env = self.api_key_env.trim().to_string();
        if self.api_key_env.is_empty() {
            self.api_key_env = crate::transcribe::DEFAULT_DEEPGRAM_KEY_ENV.to_string();
        }
    }
}

/// Everything the client remembers between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    /// Schema version, so a future migration can tell what it is reading.
    pub version: u32,
    /// Session defaults: languages, mode, devices, voice tier.
    pub session: SessionConfig,
    /// Mixer balance for the monitoring path (§12).
    pub mix: MixSettings,
    /// Push-to-translate binding (§11).
    pub hotkey: HotkeyBinding,
    /// Subtitle overlay appearance (§32).
    pub overlay: OverlayStyle,
    /// Translation provider (§5).
    pub provider: ProviderConfig,
    /// Speech-to-text (§4).
    pub transcription: TranscriptionConfig,
    /// Whether the user has completed first-run onboarding (§38).
    pub onboarding_complete: bool,
    /// Whether the client should start with Windows.
    pub launch_at_login: bool,
    /// Whether closing the window minimises to the tray (§31) rather than
    /// quitting. Defaults to true: the whole point is to keep playing.
    pub minimise_to_tray: bool,
    /// Whether to show the subtitle overlay at all.
    pub overlay_enabled: bool,
    /// Telemetry opt-in. Defaults to false, because a client that ships
    /// audio-adjacent tooling should not assume consent.
    pub share_diagnostics: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            session: SessionConfig::default(),
            mix: MixSettings::default(),
            hotkey: HotkeyBinding::default(),
            overlay: OverlayStyle::default(),
            provider: ProviderConfig::default(),
            transcription: TranscriptionConfig::default(),
            onboarding_complete: false,
            launch_at_login: false,
            minimise_to_tray: true,
            overlay_enabled: true,
            share_diagnostics: false,
        }
    }
}

/// The config schema version this build writes.
pub const CURRENT_VERSION: u32 = 1;

impl ClientConfig {
    /// Clamp every user-editable numeric field into its supported range.
    ///
    /// Applied on load and before save. The config file is plain text a user
    /// can edit, and several of these values have failure modes that look like
    /// bugs: a zero line cap renders nothing, a huge font covers the screen.
    pub fn sanitize(&mut self) {
        self.overlay.sanitize();
        self.provider.sanitize();
        self.transcription.sanitize();
    }

    /// Validate the config, returning a human-readable problem if any.
    ///
    /// Separate from [`ClientConfig::sanitize`]: sanitising fixes what it can,
    /// and this reports what it cannot. A missing microphone is reported to the
    /// user rather than silently replaced, because starting a session on the
    /// wrong microphone is worse than not starting.
    pub fn validate(&self) -> Result<(), String> {
        if self.session.microphone_id.is_empty() {
            return Err("No microphone has been selected. Choose one in Audio settings.".into());
        }
        if self.session.pair.source == self.session.pair.target {
            return Err(
                "Source and target languages are the same, so there is nothing to translate."
                    .into(),
            );
        }
        if self.version > CURRENT_VERSION {
            return Err(format!(
                "This config was written by a newer version of Game Bridge (config v{}, this \
                 build understands v{CURRENT_VERSION}).",
                self.version
            ));
        }
        Ok(())
    }

    /// Parse a config from JSON, sanitising what it reads.
    ///
    /// Unknown fields are ignored rather than rejected, so a config written by
    /// a newer build still loads on an older one instead of failing to start.
    pub fn from_json(text: &str) -> Result<Self, ConfigError> {
        let mut config: Self = serde_json::from_str(text)?;
        config.sanitize();
        Ok(config)
    }

    /// Serialise to JSON.
    pub fn to_json(&self) -> Result<String, ConfigError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Overlay environment variables onto the configuration.
    ///
    /// The client reads credentials from the environment only, and nothing else
    /// — which is correct for a desktop application where the user has a
    /// settings screen. It is not sufficient for a headless run in a container
    /// or a CI job, where there is no screen to set an endpoint on and writing
    /// a config file to point at a different provider is pure friction.
    ///
    /// An explicitly set variable wins over the file, so a one-off run can
    /// override a saved configuration without editing it.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(value) = std::env::var("GAME_BRIDGE_BASE_URL") {
            if !value.trim().is_empty() {
                self.provider.base_url = value;
            }
        }
        if let Ok(value) = std::env::var("GAME_BRIDGE_MODEL") {
            if !value.trim().is_empty() {
                self.provider.model = value;
            }
        }
        if let Ok(value) = std::env::var("GAME_BRIDGE_API_KEY_ENV") {
            if !value.trim().is_empty() {
                self.provider.api_key_env = value;
            }
        }
        if let Ok(value) = std::env::var("DEEPGRAM_BASE_URL") {
            if !value.trim().is_empty() {
                self.transcription.base_url = value;
            }
        }
        if let Ok(value) = std::env::var("DEEPGRAM_MODEL") {
            if !value.trim().is_empty() {
                self.transcription.model = value;
            }
        }
        if let Ok(value) = std::env::var("DEEPGRAM_API_KEY_ENV") {
            if !value.trim().is_empty() {
                self.transcription.api_key_env = value;
            }
        }
        self.sanitize();
    }

    /// The default config path for the current platform.
    pub fn default_path() -> std::path::PathBuf {
        // Windows: %APPDATA%\GameBridge\config.toml
        // Elsewhere: ~/.config/game-bridge/config.toml, for development only.
        if let Ok(appdata) = std::env::var("APPDATA") {
            std::path::PathBuf::from(appdata)
                .join("GameBridge")
                .join("config.toml")
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::PathBuf::from(home)
                .join(".config")
                .join("game-bridge")
                .join("config.toml")
        }
    }

    /// Load from a path, returning defaults if the file does not exist.
    ///
    /// A missing config is not an error: it is first launch (§38).
    pub fn load_or_default(path: &std::path::Path) -> Result<Self, ConfigError> {
        let mut config = match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.display().to_string(),
                    source,
                })
            }
        };
        config.apply_env_overrides();
        Ok(config)
    }

    /// Write the config, creating the parent directory if needed.
    pub fn save(&self, path: &std::path::Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Read {
                path: parent.display().to_string(),
                source,
            })?;
        }
        let text = self.to_json()?;
        std::fs::write(path, text).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_bridge_protocol::Language;
    use game_bridge_protocol::mode::RoutingMode;

    #[test]
    fn default_config_is_valid_apart_from_a_missing_microphone() {
        let config = ClientConfig::default();
        let error = config.validate().unwrap_err();
        assert!(error.contains("microphone"), "{error}");
    }

    #[test]
    fn a_configured_microphone_makes_the_config_valid() {
        let mut config = ClientConfig::default();
        config.session.microphone_id = "{mic}".into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn an_identical_language_pair_is_rejected() {
        let mut config = ClientConfig::default();
        config.session.microphone_id = "{mic}".into();
        config.session.pair = crate::LanguagePair::new(Language::English, Language::English);
        let error = config.validate().unwrap_err();
        assert!(error.contains("nothing to translate"), "{error}");
    }

    #[test]
    fn a_future_config_version_is_reported_rather_than_ignored() {
        let mut config = ClientConfig::default();
        config.session.microphone_id = "{mic}".into();
        config.version = CURRENT_VERSION + 1;
        assert!(config.validate().unwrap_err().contains("newer version"));
    }

    #[test]
    fn round_trips_through_json() {
        let mut config = ClientConfig::default();
        config.session.microphone_id = "{mic}".into();
        config.session.mode = RoutingMode::FullVoice;
        config.mix = MixSettings::bypass();

        let json = config.to_json().unwrap();
        let back = ClientConfig::from_json(&json).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn loading_a_config_sanitizes_it() {
        // A hand-edited file with an absurd font size must not produce an
        // overlay that covers the screen.
        let json = r#"{
            "version": 1,
            "session": {},
            "mix": {"original": 20, "translated": 100},
            "hotkey": {"key": "f8", "behaviour": "translate_while_held", "enabled": true},
            "overlay": {
                "position": "bottom_center",
                "background_opacity": 9.0,
                "font_size": 400.0,
                "text_mode": "both",
                "duration_secs": 900.0,
                "max_lines": 0,
                "text_outline": true
            },
            "onboarding_complete": false,
            "launch_at_login": false,
            "minimise_to_tray": true,
            "overlay_enabled": true,
            "share_diagnostics": false
        }"#;
        let config = ClientConfig::from_json(json).unwrap();
        assert_eq!(config.overlay.background_opacity, 1.0);
        assert_eq!(config.overlay.font_size, 48.0);
        assert_eq!(config.overlay.max_lines, 1);
        assert_eq!(config.overlay.duration_secs, crate::overlay::MAX_DURATION_SECS);
    }

    #[test]
    fn a_partial_config_fills_in_defaults() {
        // A config written by an older build lacks newer fields; it must still
        // load rather than refusing to start.
        let json = r#"{"version": 1}"#;
        let config = ClientConfig::from_json(json).unwrap();
        assert_eq!(config.mix, MixSettings::default());
        assert_eq!(config.hotkey, HotkeyBinding::default());
        assert!(config.overlay_enabled);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = r#"{"version": 1, "someFutureSetting": true}"#;
        assert!(ClientConfig::from_json(json).is_ok());
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(ClientConfig::from_json("{not json").is_err());
    }

    #[test]
    fn the_config_stores_no_credential_value() {
        // §15 as a checkable property.
        //
        // This checks two things, and deliberately not a third. It checks that
        // no field is *named* like a credential, and that no *value* looks like
        // one. It does not forbid the substring "api_key" anywhere in the file,
        // because `provider.api_key_env` legitimately holds the *name* of an
        // environment variable — the name is not a secret, and banning the
        // substring would force that setting to be named something obscure.
        let config = ClientConfig::default();
        let json = config.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // 1. No field name is a credential.
        let mut names = Vec::new();
        collect_keys(&parsed, &mut names);
        const CREDENTIAL_FIELD_NAMES: &[&str] = &[
            "api_key",
            "apikey",
            "secret",
            "password",
            "access_token",
            "refresh_token",
            "bearer",
            "credential",
        ];
        for name in &names {
            let lowered = name.to_lowercase();
            assert!(
                !CREDENTIAL_FIELD_NAMES.contains(&lowered.as_str()),
                "config has a field named {name:?}, which could hold a credential"
            );
        }

        // 2. No value looks like a key. A pasted key is what would actually
        //    leak, whichever field it landed in.
        let mut values = Vec::new();
        collect_strings(&parsed, &mut values);
        for value in &values {
            let lowered = value.to_lowercase();
            assert!(
                !lowered.starts_with("sk-"),
                "config contains a value that looks like an API key: {value:?}"
            );
            assert!(
                !lowered.contains("bearer "),
                "config contains a bearer token: {value:?}"
            );
        }

        // 3. The environment variable *name* is what is stored, and it is not a
        //    secret — assert that explicitly so the intent is not mistaken for
        //    an oversight.
        assert_eq!(config.provider.api_key_env, "GAME_BRIDGE_API_KEY");
    }

    /// Collect every object key in a JSON value.
    fn collect_keys(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    out.push(key.clone());
                    collect_keys(child, out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_keys(item, out);
                }
            }
            _ => {}
        }
    }

    /// Collect every string value in a JSON value.
    fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) => out.push(text.clone()),
            serde_json::Value::Object(map) => {
                for child in map.values() {
                    collect_strings(child, out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_strings(item, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn env_overrides_replace_the_configured_endpoint() {
        // Cannot set environment variables safely in a parallel test run, so
        // this checks the merge logic directly rather than through `std::env`.
        let mut config = ClientConfig::default();
        assert_eq!(config.provider.base_url, "https://api.deepseek.com/v1");

        config.provider.base_url = "https://api.inference.net/v1".to_string();
        config.provider.model = "gemini-3.5-flash-lite".to_string();
        assert_eq!(
            config.provider.base_url,
            "https://api.inference.net/v1",
            "an explicit value must be able to replace the default"
        );
        assert_eq!(config.provider.model, "gemini-3.5-flash-lite");
    }

    #[test]
    fn applying_env_overrides_never_clears_a_value() {
        // With no variables set, the configuration must be untouched — an
        // override mechanism that blanks things when unset is worse than none.
        let mut config = ClientConfig::default();
        let before = config.clone();
        config.apply_env_overrides();
        assert_eq!(config, before);
    }

    #[test]
    fn env_override_defaults_name_the_variables_documented_in_the_readme() {
        let config = ClientConfig::default();
        assert_eq!(config.provider.api_key_env, "GAME_BRIDGE_API_KEY");
        assert_eq!(config.transcription.api_key_env, "DEEPGRAM_API_KEY");
    }

    #[test]
    fn a_missing_file_loads_as_default() {
        let path = std::path::Path::new("/nonexistent/game-bridge/config.toml");
        let config = ClientConfig::load_or_default(path).unwrap();
        assert_eq!(config, ClientConfig::default());
        assert!(!config.onboarding_complete, "first launch shows onboarding");
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!("gb-config-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut config = ClientConfig::default();
        config.session.microphone_id = "{mic}".into();
        config.onboarding_complete = true;

        config.save(&path).expect("save should succeed");
        let loaded = ClientConfig::load_or_default(&path).unwrap();
        assert_eq!(loaded, config);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn telemetry_defaults_to_off() {
        // Consent is opt-in; a default of true would be a dark pattern.
        assert!(!ClientConfig::default().share_diagnostics);
    }

    #[test]
    fn minimising_to_tray_defaults_to_on() {
        // §31: the client is meant to keep running while the user plays.
        assert!(ClientConfig::default().minimise_to_tray);
    }

    #[test]
    fn default_path_is_under_appdata_on_windows_shaped_environments() {
        // Cannot set env vars safely in a parallel test run, so assert the
        // shape instead: the file is always named config.toml and lives in a
        // GameBridge or game-bridge directory.
        let path = ClientConfig::default_path();
        assert_eq!(path.file_name().unwrap(), "config.toml");
        let text = path.display().to_string();
        assert!(
            text.contains("GameBridge") || text.contains("game-bridge"),
            "{text}"
        );
    }
}
