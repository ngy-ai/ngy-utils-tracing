# ngy-utils-tracing

A Rust logging initialization utility library built on [`tracing`](https://docs.rs/tracing) that automatically selects the log output based on the "application mode", providing out-of-the-box logging configuration for development, test, and production environments.

## Features

- **Mode-driven**: Switch between `Development` / `Test` / `Production` modes via the `APP_MODE` environment variable (or code override), automatically selecting different logging configurations.
- **Console logging (development)**: `pretty` colored format with file name and line number, default timezone UTC+8, time format `YYYY-MM-DD HH:MM:SS`.
- **File logging (production)**: `JSON` format with daily rotation (`{prefix}.{date}`), convenient for collection and search.
- **Test logging**: Outputs to both console (pretty) and file (JSON), balancing readability and persistence.
- **Environment variable control**:
  - `APP_MODE`: Application mode (`development` / `test` / `production`, or shorthands `dev` / `prod`).
  - `RUST_LOG`: Log level (e.g. `info`, `debug`, or per-target `info,my_crate=debug`).
  - `LOG_DIR`: File log directory, defaults to `logs`.
  - `LOG_PREFIX`: File log prefix, defaults to `app.log`.
  - `LOG_MAX_FILES`: Max number of retained log files (including the current day's file), defaults to `7`. Older daily-rotated files beyond this are deleted at `init`, which reserves a slot for the current day's file so the directory never holds more than this limit. Long-running processes should call `cleanup_old_logs` periodically to enforce it across rotations.
  - `.env`: Auto-loaded via `dotenvy` (takes precedence over process environment variables).

> The `LOG_DIR`, `LOG_PREFIX`, and `LOG_MAX_FILES` settings can also be passed directly as
> `InitOptions` fields (`log_dir`, `log_prefix`, `max_log_files`), which take precedence over the
> environment variables.
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
    // Read mode from APP_MODE env var; crates is the list of project crates that need debug level.
    // Any field left as None falls back to its env var / default (see InitOptions).
    let options = InitOptions::default()
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

Defaults to `Production`. When `APP_MODE` or the override value is invalid, a warning is printed and it falls back to `Production`.

Resolution order: `InitOptions::mode_override` → the mode environment variable → `Production`. `.env` is loaded **before** the mode is resolved, so an `APP_MODE` defined there is honoured; since it is loaded with `dotenv_override()`, a value in `.env` takes precedence over the process environment.

## Public API

| Function / Type                  | Description                                                  |
| -------------------------------- | ------------------------------------------------------------ |
| `init(options)`                  | Unified initialization of dotenv + tracing, selecting config by mode. Accepts an [`InitOptions`](src/init.rs) struct; any `None` field falls back to its env var / default. Returns `InitResult` (with `guard`, `mode`, and `retention`). |
| `InitOptions`                    | Configuration struct for `init`. Fields: `mode_override`, `crates`, `log_dir` (overrides `LOG_DIR`), `log_prefix` (overrides `LOG_PREFIX`), `max_log_files` (overrides `LOG_MAX_FILES`, default `7`), `mode_env_var` (default `APP_MODE`), `retention_interval` (optional `Duration` for the background trim task in file modes; when `None`, the env var / `1h` default applies, and `LOG_RETENTION_INTERVAL_SECONDS=0` disables it). Provides `Default` and a chainable builder. |
| `get_current_mode()`             | Get the current application mode (requires `init()` first); returns `None` if not initialized. |
| `AppMode`                        | Application mode enum, providing `as_str` / `is_development` etc. |
| `console_tracing(crates)`        | Initialize console logging only (pretty).                    |
| `file_tracing(log_dir, log_prefix, max_log_files)` | Initialize file logging only (JSON), returning a `WorkerGuard` that must stay alive. Args override `LOG_DIR` / `LOG_PREFIX` / `LOG_MAX_FILES` when `Some`. |
| `test_tracing(crates, log_dir, log_prefix, max_log_files)` | Initialize both console and file logging, returning a `WorkerGuard`. |
| `build_debug_filter(crates)`     | Build the console `EnvFilter`: project crates default to `debug`, others `info`. |
| `build_file_filter(default)`     | Build the file `EnvFilter`, reading `RUST_LOG` first, otherwise using the default value. |
| `cleanup_old_logs(log_dir, log_prefix, max_files)` | One-shot deletion of old log files, keeping the most recent `max_files`. Only matches `{prefix}` / `{prefix}.{YYYY-MM-DD}` files, ordered by the date in the name. |
| `start_log_retention(log_dir, log_prefix, max_log_files, interval)` | Spawn a background thread that periodically calls `cleanup_old_logs` (default interval `DEFAULT_RETENTION_INTERVAL` = 1h). Returns a `LogRetentionHandle`; keep it alive (like the `WorkerGuard`) to keep the task running. |
| `DEFAULT_RETENTION_INTERVAL`      | Default `std::time::Duration` (1 hour) for `start_log_retention`. |
| `LogRetentionHandle`             | Handle for a background retention task; dropping or calling `stop()` stops the worker thread. |
| `resolve_log_dir()`              | Resolve the log directory (reads `LOG_DIR`, defaults to `logs`). |

## Environment Variables

| Variable        | Default       | Purpose                                           |
| --------------- | ------------- | ------------------------------------------------- |
| `APP_MODE`      | `production`  | Select the log mode.                              |
| `RUST_LOG`      | `info`        | Control log level, readable by `build_*_filter`.  |
| `LOG_DIR`       | `logs`        | File log directory.                               |
| `LOG_PREFIX`    | `app.log`     | File log filename prefix.                         |
| `LOG_RETENTION_INTERVAL_SECONDS` | `3600` (1h) | Background retention interval in **seconds**. `init()` starts a background task that periodically trims old log files (only for `Production`/`Test` modes); this variable defaults to `3600` (1 hour) when unset or invalid, so periodic cleanup is **enabled by default**. Set it to `0` to disable the background task. Overridden by `InitOptions::retention_interval`. |

## Notes

- **`WorkerGuard` lifetime**: In `Production` / `Test` modes, `init()` returns a `guard` that the caller must keep in `main` until the process exits, otherwise file logs may be lost.
- **One-time initialization**: The global tracing subscriber can only be set once; `init()` / `console_tracing()` / `test_tracing()` cannot be called repeatedly within a process.
- **`RUST_LOG` sharing**: In `Test` mode the console and file layers read the same `RUST_LOG`; for differentiation use per-target directives like `info,my_crate=debug`.
- **Log retention**: `init()` (via `file_tracing` / `test_tracing`) trims to `max_log_files` **once at startup**, reserving a slot for the file created for the current day so the directory never exceeds the limit. Retention only matches real rotated logs (`{prefix}` or `{prefix}.{YYYY-MM-DD}`), ordered by the date in the file name, so files like `app.logger` or `app.log.backup` are never deleted. In `Production` / `Test` modes a **background trim task runs by default** (interval from `InitOptions::retention_interval` / `LOG_RETENTION_INTERVAL_SECONDS`, default `1h`; `LOG_RETENTION_INTERVAL_SECONDS=0` disables it); its `LogRetentionHandle` is returned in `InitResult::retention`. Keep `InitResult` alive (like the file `guard`) to keep the task running. Alternatively, call `start_log_retention(...)` directly and keep its `LogRetentionHandle` alive. Both mirror the same `log_dir` / `log_prefix` / `max_log_files` resolution.

## Testing

```bash
cargo test
```

## License

[MIT](LICENSE)
