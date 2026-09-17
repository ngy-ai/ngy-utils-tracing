//! Application mode definitions: Development, Test, Production
//!
//! Controlled via the `APP_MODE` environment variable, defaulting to Production.

use std::env;
use std::fmt;
use std::str::FromStr;

/// Application run mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// Development mode: console logging, verbose output
    Development,
    /// Test mode: console + file logging
    Test,
    /// Production mode: file logging, JSON format
    #[default]
    Production,
}

impl AppMode {
    /// Get the current application mode
    ///
    /// Resolution order: prefer `override_mode`, then read the environment variable named by
    /// `mode_env_var` (e.g. `APP_MODE`), and finally fall back to the default mode
    /// (`Production`). An invalid `override_mode` is skipped with a warning so resolution
    /// continues with the environment variable; an environment value that is missing or invalid
    /// also falls back to the default mode.
    pub fn get(override_mode: Option<String>, mode_env_var: &str) -> Self {
        if let Some(s) = override_mode
            && let Some(mode) = Self::use_assign_mode(&s, "override_mode")
        {
            return mode;
        }

        if let Ok(s) = env::var(mode_env_var)
            && let Some(mode) = Self::use_assign_mode(&s, mode_env_var)
        {
            return mode;
        }

        Self::default()
    }

    /// Try to use the assigned mode string.
    ///
    /// `source` identifies where the value came from (e.g. `"override_mode"`) and is included in
    /// the warning message. Returns `None` when the value is invalid, so `get` can fall back to
    /// the next source. Note: at this point `init()` has not yet set up the tracing subscriber
    /// (see the call order in `init::init`). Using `tracing::warn!` would be silently dropped due
    /// to no subscriber, so `eprintln!` is used to ensure the warning is visible.
    fn use_assign_mode(s: &str, source: &str) -> Option<Self> {
        match Self::from_str(s) {
            Ok(mode) => Some(mode),
            Err(_) => {
                eprintln!("Invalid {} value '{}', falling back", source, s);
                None
            }
        }
    }

    /// Get the string representation of the mode
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Production => "production",
        }
    }

    /// Whether this is development mode
    pub fn is_development(&self) -> bool {
        matches!(self, Self::Development)
    }

    /// Whether this is test mode
    pub fn is_test(&self) -> bool {
        matches!(self, Self::Test)
    }

    /// Whether this is production mode
    pub fn is_production(&self) -> bool {
        matches!(self, Self::Production)
    }
}

impl fmt::Display for AppMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for AppMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "development" | "dev" => Ok(Self::Development),
            "test" => Ok(Self::Test),
            "production" | "prod" | "real" => Ok(Self::Production),
            _ => Err(format!(
                "Invalid app mode: '{}'. Valid values: development, test, production",
                s
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_production() {
        assert_eq!(AppMode::default(), AppMode::Production);
    }

    #[test]
    fn from_str_development() {
        assert_eq!(
            AppMode::from_str("development").unwrap(),
            AppMode::Development
        );
        assert_eq!(AppMode::from_str("dev").unwrap(), AppMode::Development);
    }

    #[test]
    fn from_str_test() {
        assert_eq!(AppMode::from_str("test").unwrap(), AppMode::Test);
    }

    #[test]
    fn from_str_production() {
        assert_eq!(
            AppMode::from_str("production").unwrap(),
            AppMode::Production
        );
        assert_eq!(AppMode::from_str("prod").unwrap(), AppMode::Production);
    }

    #[test]
    fn from_str_invalid() {
        assert!(AppMode::from_str("invalid").is_err());
    }

    #[test]
    fn as_str_roundtrip() {
        assert_eq!(
            AppMode::from_str(AppMode::Development.as_str()).unwrap(),
            AppMode::Development
        );
        assert_eq!(
            AppMode::from_str(AppMode::Test.as_str()).unwrap(),
            AppMode::Test
        );
        assert_eq!(
            AppMode::from_str(AppMode::Production.as_str()).unwrap(),
            AppMode::Production
        );
    }

    #[test]
    fn is_methods() {
        assert!(AppMode::Development.is_development());
        assert!(!AppMode::Development.is_test());
        assert!(!AppMode::Development.is_production());

        assert!(!AppMode::Test.is_development());
        assert!(AppMode::Test.is_test());
        assert!(!AppMode::Test.is_production());

        assert!(!AppMode::Production.is_development());
        assert!(!AppMode::Production.is_test());
        assert!(AppMode::Production.is_production());
    }

    #[test]
    fn display() {
        assert_eq!(format!("{}", AppMode::Development), "development");
        assert_eq!(format!("{}", AppMode::Test), "test");
        assert_eq!(format!("{}", AppMode::Production), "production");
    }
}
