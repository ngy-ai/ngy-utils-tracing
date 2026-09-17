//! Timestamp offset shared by the console and file layers.
//!
//! Both layers render the same instant with the same UTC offset and the same RFC 3339 format, so
//! the two outputs can be read side by side. Each layer samples the clock while formatting, so the
//! sub-second digits of one event may differ slightly between the terminal and the file.
//!
//! Offset precedence: [`InitOptions::time_offset`](crate::InitOptions::time_offset) → the
//! `LOG_TIME_OFFSET` environment variable → [`crate::DEFAULT_TIME_OFFSET`].

use time::{UtcOffset, format_description::well_known::Rfc3339};
use tracing_subscriber::fmt::time::OffsetTime;

/// Suffix of the environment variable holding the UTC offset used for log timestamps (e.g.
/// `+08:00`); the full name is prefixed with [`InitOptions::env_prefix`](crate::InitOptions::env_prefix).
const TIME_OFFSET_ENV: &str = "LOG_TIME_OFFSET";

/// Timer shared by the console and file layers.
///
/// Renders RFC 3339 with the resolved offset, e.g. `2026-09-17T19:03:04.123456+08:00` (or `...Z`
/// when the offset is UTC), so the console and the JSON file are directly comparable. Sharing the
/// timer also keeps the format from drifting between the two layers.
pub(crate) fn rfc3339_timer(time_offset: UtcOffset) -> OffsetTime<Rfc3339> {
    OffsetTime::new(time_offset, Rfc3339)
}

/// Resolve the UTC offset used for log timestamps.
///
/// The same offset is used by every layer, so the console and the JSON file agree on the
/// wall-clock time of a record.
///
/// An unparsable value is reported on stderr and skipped so that the next source applies. This
/// runs before the subscriber is installed, where `tracing` would drop a warning, so `eprintln!`
/// is used instead — mirroring [`AppMode::get`](crate::AppMode::get).
pub(crate) fn resolve_time_offset(env_prefix: &str, override_value: Option<&str>) -> UtcOffset {
    if let Some(raw) = override_value
        && let Some(offset) = use_offset(raw, "InitOptions::time_offset")
    {
        return offset;
    }

    let name = crate::env_var_name(env_prefix, TIME_OFFSET_ENV);
    if let Ok(raw) = std::env::var(&name)
        && let Some(offset) = use_offset(&raw, &name)
    {
        return offset;
    }

    // `DEFAULT_TIME_OFFSET` is a constant, so falling back to UTC is only a safety net for a
    // future edit that makes it unparsable.
    parse_offset(crate::DEFAULT_TIME_OFFSET).unwrap_or(UtcOffset::UTC)
}

/// Parse an offset on the next source, warning when the value is unusable.
fn use_offset(raw: &str, source: &str) -> Option<UtcOffset> {
    match parse_offset(raw) {
        Some(offset) => Some(offset),
        None => {
            eprintln!("Invalid {source} value '{raw}', falling back");
            None
        }
    }
}

/// Parse an offset written as `UTC`/`Z`, `±HH` or `±HH:MM`.
///
/// The sign is mandatory so that `+8` and `08:00` cannot be confused. The magnitude is checked
/// against the RFC 3339 range (±23:59) before the sign is applied, because `UtcOffset::from_hms`
/// is looser and an out-of-range offset would render unusable RFC 3339 file timestamps.
fn parse_offset(value: &str) -> Option<UtcOffset> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("utc") || value.eq_ignore_ascii_case("z") {
        return Some(UtcOffset::UTC);
    }

    let (sign, rest) = match value.as_bytes().first()? {
        b'+' => (1, &value[1..]),
        b'-' => (-1, &value[1..]),
        _ => return None,
    };
    let (hours, minutes) = match rest.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None => (rest, "0"),
    };
    let hours: i8 = hours.trim().parse().ok()?;
    let minutes: i8 = minutes.trim().parse().ok()?;
    if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
        return None;
    }

    UtcOffset::from_hms(sign * hours, sign * minutes, 0).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prefix used by the tests so they never touch a variable name a real deployment could use.
    const TEST_PREFIX: &str = "NGY_TEST_";

    fn offset(hours: i8, minutes: i8) -> UtcOffset {
        UtcOffset::from_hms(hours, minutes, 0).unwrap()
    }

    #[test]
    fn parse_offset_accepts_the_documented_forms() {
        assert_eq!(parse_offset("+08:00"), Some(offset(8, 0)));
        assert_eq!(parse_offset("+8"), Some(offset(8, 0)));
        assert_eq!(parse_offset("-05:30"), Some(offset(-5, -30)));
        assert_eq!(parse_offset(" +08:00 "), Some(offset(8, 0)));
        assert_eq!(parse_offset("UTC"), Some(UtcOffset::UTC));
        assert_eq!(parse_offset("z"), Some(UtcOffset::UTC));
    }

    #[test]
    fn parse_offset_rejects_unusable_forms() {
        for value in [
            "",
            "08:00",
            "eight",
            "+08:60",
            "+25:00",
            "+08:00:00",
            "+08:-30",
        ] {
            assert_eq!(parse_offset(value), None, "value: {value:?}");
        }
    }

    #[test]
    fn resolve_time_offset_prefers_the_override() {
        // The override path returns before touching the environment, so this test needs no lock.
        assert_eq!(
            resolve_time_offset(TEST_PREFIX, Some("-05:30")),
            offset(-5, -30)
        );
        assert_eq!(
            resolve_time_offset(TEST_PREFIX, Some("UTC")),
            UtcOffset::UTC
        );
    }

    #[test]
    fn resolve_time_offset_reads_the_prefixed_variable() {
        let _lock = crate::TEST_ENV_MUTEX.lock().unwrap();
        let name = crate::env_var_name(TEST_PREFIX, TIME_OFFSET_ENV);
        // SAFETY: modifying the process environment is unsafe in edition 2024; holding
        // crate::TEST_ENV_MUTEX serializes this with every other test that touches env vars.
        unsafe { std::env::set_var(&name, "+05:30") };

        assert_eq!(resolve_time_offset(TEST_PREFIX, None), offset(5, 30));

        // SAFETY: Same as above; restore the environment variable set by this test.
        unsafe { std::env::remove_var(&name) };
    }

    #[test]
    fn resolve_time_offset_ignores_the_unprefixed_variable() {
        let _lock = crate::TEST_ENV_MUTEX.lock().unwrap();
        // An unrelated program is free to set the unprefixed name; it must not reach us.
        // SAFETY: modifying the process environment is unsafe in edition 2024; holding
        // crate::TEST_ENV_MUTEX serializes this with every other test that touches env vars.
        unsafe { std::env::set_var(TIME_OFFSET_ENV, "+05:30") };

        assert_eq!(resolve_time_offset(TEST_PREFIX, None), offset(8, 0));

        // SAFETY: Same as above; restore the environment variable set by this test.
        unsafe { std::env::remove_var(TIME_OFFSET_ENV) };
    }
}
