# ngy-utils-tracing

A Rust logging initialization utility library built on [`tracing`](https://docs.rs/tracing) that automatically selects the log output based on the "application mode", providing out-of-the-box logging configuration for development, test, and production environments.

## Features

- **Mode-driven**: Switch between `Development` / `Test` / `Production` modes via the prefixed `APP_MODE` environment variable (or code override), automatically selecting different logging configurations.
- **Console logging (development)**: `pretty` format with file name, line number, thread ids/names and span close events; colors only when stdout is a terminal, RFC 3339 timestamps (offset configurable via the prefixed `LOG_TIME_OFFSET`, default `+08:00`).
- **File logging (production)**: `JSON` format with daily rotation (`{prefix}.{date}`), convenient for collection and search.
- **Test logging**: Outputs to both console (pretty) and file (JSON), balancing readability and persistence.
- **Prefixed, collision-free configuration**: every variable is read as `env_prefix` + name, so with `InitOptions::new("NGY_")` the log directory comes from `NGY_LOG_DIR` and an unrelated program's `LOG_DIR` is never touched. `RUST_LOG` is the one exception and stays unprefixed, because the Rust logging ecosystem shares it. See [Environment Variables](#environment-variables). `.env` is auto-loaded via `dotenvy` and takes precedence over the process environment.

> Every setting can also be passed as an `InitOptions` field, which overrides the corresponding
> environment variable; the prefix itself is required and is passed to `InitOptions::new`. The
> field docs are the authoritative description of each option; the tables in this README are a
> summary.
- **Process-wide one-time initialization**: `init()` can only be called once per process; repeated calls return a clear error.

## Installation

Add to `Cargo.toml`:

```toml
[dependencies]
ngy-utils-tracing = "0.1"
```

> Requires Rust 2024 edition (toolchain locked to `stable` via `rust-toolchain.toml`).

## Quick Start

```rust
use ngy_utils_tracing::{InitOptions, init, get_current_mode};

fn main() -> anyhow::Result<()> {
    // `NGY_` is the required env prefix: the mode comes from NGY_APP_MODE, the log directory from
    // NGY_LOG_DIR, and so on. `crates` lists the project crates that need the debug level.
    // Any field left as None falls back to its env var / default (see InitOptions).
    let options = InitOptions::new("NGY_")
        .crates(vec!["my_crate".to_string()])
        .log_dir(Some("logs".to_string()))
        .log_prefix(Some("app.log".to_string()))
        .max_log_files(Some(7));
    let result = init(options)?;

    println!("started in {} mode", result.mode);
    tracing::info!("Application started");

    // File logging scenario: ensure the guard stays alive for the process lifetime, otherwise logs may be lost
    if let Some(_guard) = result.guard {
        // guard should be retained until main ends
    }

    // The current mode can be queried at any time afterwards
    if let Some(mode) = get_current_mode() {
        tracing::info!("current mode: {}", mode);
    }

    Ok(())
}
```

## Modes

| Mode            | Values (case-insensitive)     | Log output                           |
| --------------- | ----------------------------- | ------------------------------------ |
| `Development`   | `development` / `dev`         | Console (pretty format)              |
| `Test`          | `test`                        | Console (pretty) + File (JSON)       |
| `Production`    | `production` / `prod`         | File (JSON, daily rotation)          |

Defaults to `Production`. An invalid override value is skipped (with a warning) so resolution continues with the environment variable; when `{prefix}APP_MODE` is missing or invalid, it falls back to `Production`.

Resolution order: `InitOptions::mode_override` → the mode environment variable (the prefixed `APP_MODE` by default, or `InitOptions::mode_env_var` used verbatim) → `Production`. `.env` is loaded **before** the mode is resolved, so a prefixed `APP_MODE` defined there is honoured; since it is loaded with `dotenv_override()`, a value in `.env` takes precedence over the process environment.

## Public API

| Function / Type                  | Description                                                  |
| -------------------------------- | ------------------------------------------------------------ |
| `init(options)`                  | Unified initialization of dotenv + tracing, selecting config by mode. Accepts an `InitOptions` struct; any `None` field falls back to its env var / default. Returns `InitResult` (with `guard`, `mode`, and `retention`). |
| `InitOptions`                    | Configuration struct for `init`. Fields: `env_prefix` (**required**, see below), `mode_override`, `crates`, `log_dir`, `log_prefix`, `max_log_files`, `mode_env_var`, `retention_interval`, `time_offset` — each one overrides the matching environment variable, and the field docs document the default. Created with `InitOptions::new(prefix)` plus a chainable builder; there is no `Default`, so the prefix cannot be forgotten. |
| `get_current_mode()`             | Get the current application mode (requires `init()` first); returns `None` if not initialized. |
| `AppMode`                        | Application mode enum, providing `as_str` / `is_development` etc. |
| `console_tracing(env_prefix, crates)` | Initialize console logging only (pretty, RFC 3339 timestamps; colors only on a terminal). |
| `file_tracing(env_prefix, log_dir, log_prefix, max_log_files)` | Initialize file logging only (JSON, RFC 3339 timestamps), returning a `WorkerGuard` that must stay alive. Args override the prefixed `LOG_DIR` / `LOG_PREFIX` / `LOG_MAX_FILES` when `Some`. |
| `test_tracing(env_prefix, crates, log_dir, log_prefix, max_log_files)` | Initialize both console and file logging with the same directive list, returning a `WorkerGuard`. |
| `build_debug_filter(crates)`     | Build the console `EnvFilter`: project crates default to `debug`, others `info`, with `RUST_LOG` directives layered on top (they win for the targets they name). |
| `build_file_filter(default)`     | Build the file `EnvFilter`, reading `RUST_LOG` first, otherwise using the default value. |
| `cleanup_old_logs(log_dir, log_prefix, max_files)` | One-shot deletion of old log files, keeping the most recent `max_files`. Only matches `{prefix}` / `{prefix}.{YYYY-MM-DD}` files, ordered by the date in the name. |
| `start_log_retention(env_prefix, log_dir, log_prefix, max_log_files, interval)` | Spawn a background thread that periodically calls `cleanup_old_logs` (default interval `DEFAULT_RETENTION_INTERVAL` = 1h). Returns a `LogRetentionHandle`; keep it alive (like the `WorkerGuard`) to keep the task running. |
| `DEFAULT_LOG_LEVEL`, `DEFAULT_LOG_DIR`, `DEFAULT_LOG_PREFIX`, `DEFAULT_MAX_LOG_FILES`, `DEFAULT_RETENTION_INTERVAL`, `DEFAULT_TIME_OFFSET` | Built-in defaults used when the matching environment variable is unset; referenced by the `InitOptions` field docs. |
| `LogRetentionHandle`             | Handle for a background retention task; dropping or calling `stop()` stops the worker thread. |
| `resolve_log_dir(env_prefix)`    | Resolve the log directory (reads the prefixed `LOG_DIR`, falls back to `DEFAULT_LOG_DIR`). Also `resolve_log_prefix`, `resolve_max_log_files`, `resolve_retention_interval`. |

## Environment Variables

Quick reference only — the authoritative description of each variable, the `InitOptions` field that
overrides it and the exact default lives on
[`InitOptions`](https://docs.rs/ngy-utils-tracing/latest/ngy_utils_tracing/struct.InitOptions.html).

Every variable below is read as `env_prefix` + the name, where `env_prefix` is the **required**
prefix passed to `InitOptions::new`. With `InitOptions::new("NGY_")` the log directory therefore
comes from `NGY_LOG_DIR`, and the unprefixed `LOG_DIR` is never read — an unrelated program that
uses `LOG_DIR` or `APP_MODE` for its own purposes cannot change this crate's behaviour. `RUST_LOG`
is the one exception: it keeps its unprefixed name because the whole Rust logging ecosystem shares
it.

| Variable        | Default       | Purpose                                           |
| --------------- | ------------- | ------------------------------------------------- |
| `{prefix}APP_MODE` | `production` | Select the log mode (`development` / `test` / `production`, `dev` / `prod` shorthand). |
| `RUST_LOG`      | `info`        | Log level or per-target directives; layered on top of `crates` (see the notes below). Never prefixed. |
| `{prefix}LOG_DIR` | `logs`      | File log directory.                               |
| `{prefix}LOG_PREFIX` | `app.log` | File log filename prefix; daily files are `{prefix}.{YYYY-MM-DD}`. |
| `{prefix}LOG_MAX_FILES` | `7`    | Max number of retained log files, including the current day's; older daily-rotated files are deleted at `init`. |
| `{prefix}LOG_RETENTION_INTERVAL_SECONDS` | `3600` | Background retention interval in **seconds**; `0` disables the background task. |
| `{prefix}LOG_TIME_OFFSET` | `+08:00` | UTC offset for log timestamps (e.g. `-05:30`, `UTC`); shared by the console and file layers. |

## Notes

- **`WorkerGuard` lifetime**: In `Production` / `Test` modes, `init()` returns a `guard` that the caller must keep in `main` until the process exits, otherwise file logs may be lost.
- **One-time initialization**: The global tracing subscriber can only be set once; `init()` / `console_tracing()` / `test_tracing()` cannot be called repeatedly within a process, and they fail with a clear error if another library already installed a global subscriber.
- **`log` interoperability**: `init()` installs a `log` compatibility layer so dependencies that log through `log` are captured. If the application already installed a `log` logger (e.g. `env_logger`), that layer is skipped with a warning on stderr instead of failing initialization; the tracing subscriber is still installed.
- **`RUST_LOG` precedence**: `build_debug_filter` / `console_tracing` first turn `crates` into `my_crate=debug` directives plus an `info` fallback, then append the directives from `RUST_LOG` on top. `EnvFilter` lets the last directive for a target win, so `RUST_LOG=hyper=warn` only quiets `hyper` while the project crates stay at `debug`, `RUST_LOG=my_crate=trace` overrides a single crate, and a bare level such as `RUST_LOG=warn` replaces the global `info` default. `RUST_LOG` therefore no longer discards the `crates` directives as a whole, and in `Test` mode the file layer is given the same directive list so both outputs log the same records. No dependency is excluded by the library itself, so quiet a noisy one with a per-target directive such as `RUST_LOG=sqlx=warn`. `RUST_LOG` is the only variable that is never prefixed, because the whole Rust logging ecosystem shares it.
- **Timestamps**: both layers print RFC 3339 with the offset from `LOG_TIME_OFFSET` / `InitOptions::time_offset` (`2026-09-17T19:03:04.123456+08:00`, or `...Z` for `UTC`), so the console and the JSON file can be read side by side. Each layer samples the clock while formatting, so the sub-second digits can differ slightly between the two for the same event.
- **Console colors**: enabled only when stdout is a terminal, so redirected output, pipes and CI stay free of escape codes; `NO_COLOR` is honoured.
- **Log retention**: Retention only matches real rotated logs (`{prefix}` or `{prefix}.{YYYY-MM-DD}`), ordered by the date in the file name, so files like `app.logger` or `app.log.backup` are never deleted. `init()` trims once at startup, and in `Production` / `Test` a background task keeps trimming (see `LOG_RETENTION_INTERVAL_SECONDS`); its `LogRetentionHandle` is returned in `InitResult::retention` and must stay alive like the file `guard`. Alternatively call `start_log_retention(...)` directly and keep its handle alive.

## Testing

```bash
cargo test
```

## License

[MIT](LICENSE)
