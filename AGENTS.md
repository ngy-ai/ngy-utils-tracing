# AGENTS.md

Guidance for AI agents and contributors working on this repository.

## Project overview

`ngy-utils-tracing` is a Rust logging-initialization utility built on `tracing`. It selects the
log output (console / file / both) from an "application mode" and provides file log rotation with
retention.

- `Development` — console logging only (pretty format)
- `Test` — console (pretty) + file (JSON)
- `Production` — file logging only (JSON, daily rotation)

## Repository layout

| Path | Purpose |
| --- | --- |
| `src/lib.rs` | Crate root, public re-exports, test-only `TEST_ENV_MUTEX` |
| `src/app_mode.rs` | `AppMode` enum and env resolution |
| `src/console_tracing.rs` | Console layer / filters |
| `src/file_tracing.rs` | File layer, `cleanup_old_logs`, `start_log_retention`, env resolvers |
| `src/test_tracing.rs` | Console + file combination |
| `src/init.rs` | `init()`, `InitOptions`, `InitResult`, `get_current_mode()` |
| `tests/` | Integration tests (see testing notes below) |

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked --all
```

Run all three before committing. CI runs exactly these (plus `cargo publish`, see below).

## Conventions

- Rust edition 2024; formatting is enforced by `rustfmt.toml` (run `cargo fmt`).
- Write code comments, doc comments, README text and commit messages in **English**.
- Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `chore:`,
  `ci:`, `docs:`) — `feat!:` / `BREAKING CHANGE` for incompatible API changes.
- New public items must be re-exported from `src/lib.rs` and documented with a `///` doc comment.
- Keep `README.md` in sync when behavior, environment variables or the public API change.

## Release process (IMPORTANT)

**Never run `cargo publish` locally. Publishing is automated by CI.**

`.github/workflows/publish.yml` publishes the crate to crates.io and is triggered **only** by:

- pushing a tag matching `v*` (e.g. `v0.5.0`), or
- a manual `workflow_dispatch` run from the GitHub Actions UI.

Pushing ordinary commits to `main` **does not** publish anything.

To release a new version:

1. Bump `version` in `Cargo.toml` following semver (`0.x`: minor bump for new features / API,
   patch for fixes only).
2. Refresh the lockfile and verify locally:
   `cargo check --all-targets && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --locked --all`
3. Update `README.md` if the change affects documented behavior.
4. Commit, then tag and push the tag:
   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
5. CI runs format / clippy / test, then `cargo publish --token $CARGO_REGISTRY_TOKEN`.
   The token comes from the repository secret `CARGO_REGISTRY_TOKEN`.

Rules and pitfalls:

- Do **not** push a tag for a version that already exists on crates.io — the CI publish step will
  fail with `crate version already exists`.
- Do **not** `git push --force` a tag that has already triggered a release.
- If a release must be redone, bump to the next version instead of reusing the old one.

## Testing notes

- The global tracing subscriber can be set only once per process, so every `init()` scenario lives
  in its own integration test file (`tests/init_*.rs`). Add a new file rather than a new `#[test]`
  when it needs its own `init()`.
- Tests that read or write environment variables are serialized by `crate::TEST_ENV_MUTEX`; hold it
  for the whole test. Modifying env vars is `unsafe` in edition 2024 — keep it inside an
  `unsafe { ... }` block with a `SAFETY`-style comment.
- `tests/init_test.rs` and `tests/init_env_retention_test.rs` rely on process env vars; keep them in
  separate files so they never run concurrently with other env-modifying tests.

## Environment variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `APP_MODE` | `production` | Log mode: `development` / `test` / `production` (`dev` / `prod` shorthand) |
| `RUST_LOG` | `info` | Log level / per-target directives |
| `LOG_DIR` | `logs` | File log directory |
| `LOG_PREFIX` | `app.log` | File log name prefix (daily files are `{prefix}.{YYYY-MM-DD}`) |
| `LOG_MAX_FILES` | `7` | Max retained log files, including the current day's file |
| `LOG_RETENTION_INTERVAL_SECONDS` | `3600` | Background trim interval; `0` disables the background task |

`.env` is loaded via `dotenvy` with `dotenv_override()`, so it takes precedence over the process
environment. `InitOptions` fields override the corresponding environment variables.
