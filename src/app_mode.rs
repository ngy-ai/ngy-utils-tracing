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
    /// `mode_env_var` (e.g. `APP_MODE`), falling back to Production when both are missing or
    /// invalid. When `override_mode` or the env var value is invalid, a warning is printed and it
    /// falls back to Production.
    pub fn get(override_mode: Option<String>, mode_env_var: &str) -> Self {
        if let Some(s) = override_mode {
            match Self::from_str(&s) {
                Ok(mode) => return mode,
                Err(_) => {
                    eprintln!(
                        "Invalid override_mode value '{}', falling back to Production",
                        s
                    );
                    return Self::Production;
                }
            }
        }

        match env::var(mode_env_var) {
            Ok(s) => match Self::from_str(&s) {
                Ok(mode) => mode,
                Err(_) => {
                    // Note: at this point init() has not yet set up the tracing subscriber
                    // (see the call order in init::init). Using tracing::warn! would be silently
                    // dropped due to no subscriber. Use eprintln! to ensure the warning is visible.
                    eprintln!(
                        "Invalid {} value '{}', falling back to Production",
                        mode_env_var, s
                    );
                    Self::Production
                }
            },
            Err(_) => Self::Production,
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
            "production" | "prod" => Ok(Self::Production),
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
