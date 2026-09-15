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
  - `.env`: Auto-loaded via `dotenvy` (takes precedence over process environment variables).
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
use ngy_utils_tracing::{init, get_current_mode};

fn main() -> anyhow::Result<()> {
    // Read mode from APP_MODE env var; crates is the list of project crates that need debug level
    let result = init(None, vec!["my_crate".to_string()], None)?;

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

Defaults to `Development`. When `APP_MODE` or the override value is invalid, a warning is printed and it falls back to `Development`.

## Public API

| Function / Type                  | Description                                                  |
| -------------------------------- | ------------------------------------------------------------ |
| `init(mode_override, crates, log_prefix_name)` | Unified initialization of dotenv + tracing, selecting config by mode. `log_prefix_name` optionally overrides the file name prefix. Returns `InitResult` (with `guard` and `mode`). |
| `get_current_mode()`             | Get the current application mode (requires `init()` first); returns `None` if not initialized. |
| `AppMode`                        | Application mode enum, providing `as_str` / `is_development` etc. |
| `console_tracing(crates)`        | Initialize console logging only (pretty).                    |
| `file_tracing()`                 | Initialize file logging only (JSON), returning a `WorkerGuard` that must stay alive. |
| `test_tracing(crates)`           | Initialize both console and file logging, returning a `WorkerGuard`. |
| `build_debug_filter(crates)`     | Build the console `EnvFilter`: project crates default to `debug`, others `info`. |
| `build_file_filter(default)`     | Build the file `EnvFilter`, reading `RUST_LOG` first, otherwise using the default value. |
| `resolve_log_dir()`              | Resolve the log directory (reads `LOG_DIR`, defaults to `logs`). |

## Environment Variables

| Variable        | Default       | Purpose                                           |
| --------------- | ------------- | ------------------------------------------------- |
| `APP_MODE`      | `development` | Select the log mode.                              |
| `RUST_LOG`      | `info`        | Control log level, readable by `build_*_filter`.  |
| `LOG_DIR`       | `logs`        | File log directory.                               |
| `LOG_PREFIX`    | `app.log`     | File log filename prefix.                         |

## Notes

- **`WorkerGuard` lifetime**: In `Production` / `Test` modes, `init()` returns a `guard` that the caller must keep in `main` until the process exits, otherwise file logs may be lost.
- **One-time initialization**: The global tracing subscriber can only be set once; `init()` / `console_tracing()` / `test_tracing()` cannot be called repeatedly within a process.
- **`RUST_LOG` sharing**: In `Test` mode the console and file layers read the same `RUST_LOG`; for differentiation use per-target directives like `info,my_crate=debug`.

## Testing

```bash
cargo test
```

## License

[MIT](LICENSE)
