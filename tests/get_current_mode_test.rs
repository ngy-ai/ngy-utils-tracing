//! get_current_mode integration tests
//!
//! Verify that get_current_mode() returns None before init() is called.
//! This file deliberately does not call init() to keep an independent process for testing the
//! "uninitialized" state.

use ngy_utils_tracing::get_current_mode;

#[test]
fn get_current_mode_returns_none_before_init() {
    // init() has not been called in this process; the global mode is unset
    assert_eq!(get_current_mode(), None);
}
