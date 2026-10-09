// Modified copy of alacritty/src/config/mod.rs at def47a5dd511ed20607eb5d9e5e7140afd16b31d.
//
// Changes for the embedded host:
// - Configuration files are not discovered, imported, watched or migrated. The embedder
//   supplies one TOML document which is deserialized with the unmodified upstream
//   `UiConfig` deserializer, followed by the same `after_loading` step upstream runs
//   (hint binding generation). CLI overrides do not exist.
// - `LOG_TARGET_CONFIG` moved here from the removed `logging` module.
// - The `monitor` module (live reload) is not included.

use std::fmt::{self, Display, Formatter};

use serde::Deserialize;
use toml::de::Error as TomlError;
use toml::Value;

pub mod bell;
pub mod color;
pub mod cursor;
pub mod debug;
pub mod font;
pub mod general;
pub mod scrolling;
pub mod selection;
pub mod serde_utils;
pub mod terminal;
pub mod ui_config;
pub mod window;

mod bindings;
mod mouse;

pub use crate::config::bindings::{
    Action, BindingKey, BindingMode, MouseAction, SearchAction, ViAction,
};
pub use crate::config::ui_config::UiConfig;

/// Logging target for config error messages (upstream `logging::LOG_TARGET_CONFIG`).
pub const LOG_TARGET_CONFIG: &str = "alacritty_config_derive";

/// Result from config loading.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors occurring during config loading.
#[derive(Debug)]
pub enum Error {
    /// Invalid toml.
    Toml(TomlError),
    /// embed: Semantically invalid values must not enter the renderer during reload.
    Invalid(&'static str),
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Toml(err) => err.source(),
            Error::Invalid(_) => None,
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Error::Toml(err) => write!(f, "Config error: {err}"),
            Error::Invalid(err) => write!(f, "Config error: {err}"),
        }
    }
}

impl From<TomlError> for Error {
    fn from(val: TomlError) -> Self {
        Error::Toml(val)
    }
}

/// Deserialize an embedder-provided TOML configuration document.
///
/// This mirrors upstream `read_config` + `after_loading` for a single document without
/// imports or CLI overrides.
pub fn from_toml(contents: &str) -> Result<UiConfig> {
    let value: Value = toml::from_str(contents)?;
    // embed: Upstream FontSize quantizes/clamps numbers; reject invalid source values
    // before that conversion can turn NaN/negative/zero into a seemingly valid size.
    if let Some(size) = value.get("font").and_then(|font| font.get("size")) {
        let number = size.as_float().or_else(|| size.as_integer().map(|number| number as f64));
        if number.is_some_and(|number| !number.is_finite() || number <= 0.) {
            return Err(Error::Invalid("font.size must be a positive finite number"));
        }
    }
    // embed: Preserve upstream defaulting for omitted fields but reject invalid values.
    let mut config = alacritty_config::with_strict(|| UiConfig::deserialize(value))?;
    if config.font.size().as_px() <= 0. {
        return Err(Error::Invalid("font.size is too small to render"));
    }

    // Create key bindings for regex hints.
    config.generate_hint_bindings();

    Ok(config)
}

// embed: Reload validation runs without initializing GTK/AppKit/GL or spawning a child.
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_reload_without_changing_previous_config() {
        let good = from_toml("[font]\nsize = 15\n[scrolling]\nhistory = 123").unwrap();
        for invalid in ["[font", "[font]\nsize = 'large'", "[font.normal]\nfamily = 3",
            "[scrolling]\nhistory = 'lots'", "[cursor]\nstyle = 'wrong'",
            "[font]\nsize = -1", "[font]\nsize = 0", "[font]\nsize = nan",
            "[[keyboard.bindings]]\nkey = 'Space'\nmods = 'Control|Shift'\naction = 'wrong'"]
        {
            assert!(from_toml(invalid).is_err(), "accepted {invalid}");
        }
        assert_eq!(good.font.size(), from_toml("[font]\nsize = 15").unwrap().font.size());
        assert!(from_toml("[font]\nsize = 18\n[font.normal]\nfamily = 'monospace'").is_ok());
    }
}
