//! Initialization module integration tests
//!
//! Tests the individual components of the init flow.
//! Note: since tracing's global subscriber can only be set once,
//! the full init() tests are placed in separate files (one per mode).

use ngy_utils_tracing::AppMode;
use std::env;
use std::sync::Mutex;

// Serialize all tests that modify the APP_MODE environment variable to avoid parallel conflicts
static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn test_app_mode_from_env() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // SAFETY: Holding the ENV_MUTEX lock, the cases that modify APP_MODE in this test binary are
    // serialized, so there is no race with concurrent read/write of the process env var by other tests.
    unsafe {
        env::set_var("APP_MODE", "production");
    }
    assert_eq!(AppMode::get(None), AppMode::Production);

    unsafe {
        env::set_var("APP_MODE", "test");
    }
    assert_eq!(AppMode::get(None), AppMode::Test);

    unsafe {
        env::set_var("APP_MODE", "development");
    }
    assert_eq!(AppMode::get(None), AppMode::Development);

    unsafe {
        env::remove_var("APP_MODE");
    }
    assert_eq!(AppMode::get(None), AppMode::Development);
}

#[test]
fn test_app_mode_aliases() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // SAFETY: Same as above; holding the ENV_MUTEX lock ensures this case exclusively modifies APP_MODE.
    unsafe {
        env::set_var("APP_MODE", "dev");
    }
    assert_eq!(AppMode::get(None), AppMode::Development);

    unsafe {
        env::set_var("APP_MODE", "prod");
    }
    assert_eq!(AppMode::get(None), AppMode::Production);

    unsafe {
        env::remove_var("APP_MODE");
    }
}

#[test]
fn test_app_mode_invalid_falls_back_to_development() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // SAFETY: Same as above; holding the ENV_MUTEX lock ensures this case exclusively modifies APP_MODE.
    unsafe {
        env::set_var("APP_MODE", "invalid_mode");
    }
    assert_eq!(AppMode::get(None), AppMode::Development);
    unsafe {
        env::remove_var("APP_MODE");
    }
}
