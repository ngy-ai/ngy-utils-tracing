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
| `src/lib.rs` | Crate root, public re-exports, crate-wide `DEFAULT_*` constants, `warn`, test-only `TEST_ENV_MUTEX` |
| `src/app_mode.rs` | `AppMode` enum and env resolution |
| `src/console_tracing.rs` | Console layer / filters |
| `src/file_tracing.rs` | File layer: resolves the file-log settings, builds the layer and its writer |
| `src/rolling_file.rs` | The daily-rotating file writer: file names follow the timestamp offset, retention runs at rotation |
| `src/retention.rs` | `cleanup_old_logs`: which files count as logs, how they are ordered, which of them are deleted |
| `src/test_tracing.rs` | Console + file combination (both layers share one directive list) |
| `src/timestamp.rs` | `LOG_TIME_OFFSET` resolution (the machine's time zone by default) and the RFC 3339 timer shared by both layers |
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
- Install the tracing subscriber through `crate::install_subscriber`, **never**
  `SubscriberInitExt::try_init`: the latter treats a pre-existing `log` logger (an application using
  `env_logger`, say) as a fatal error, while we install our subscriber anyway and only skip the
  `log` bridge, with a warning. An already installed *tracing* subscriber stays a hard error.

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
- Tests that read or write environment variables in the **lib** test binary are serialized by
  `crate::TEST_ENV_MUTEX`; hold it for the whole test. Each integration test binary is a separate
  process, so it uses a file-local mutex instead (`tests/tracing_test.rs`).
  Modifying env vars is `unsafe` in edition 2024 — keep it inside an `unsafe { ... }` block with a
  `SAFETY`-style comment.
- Prefer `tests/*_test.rs` for behaviour the public API can express (`cleanup_old_logs`, the
  `*_filter` builders, …): those tests double as a consumer-side contract. Keep a test inside the
  module only when it needs a crate-internal item (e.g. something `pub(crate)`), which is why the
  rotation, file-naming and retention-at-rotation tests live in `src/rolling_file.rs`: they drive a
  crate-private writer with a fixed clock instead of waiting for midnight.
- `tests/init_test.rs` relies on process env vars; keep it in its own file so it never runs
  concurrently with other env-modifying tests. `tests/init_production_test.rs` writes into the
  default `logs/` directory (excluded from the package in `Cargo.toml`) and reads the newest file
  back to check that its name repeats the date its timestamp shows.
- `tests/docs_consistency_test.rs` fails when the README environment table drifts from the
  `DEFAULT_*` constants, when a variable is missing from the `InitOptions` docs, or when
  `AGENTS.md` grows a second copy of that table.
- Tests never touch an unprefixed variable name: they pass a test-only prefix (e.g. `NGY_TEST_`,
  usually via a local `const ENV_PREFIX`) so they cannot read or clobber a name a real deployment
  owns. Keep the "unprefixed decoy is ignored" assertions when adding env-driven tests.

## Environment variables

Do **not** keep a second copy of the variable table here. The authoritative description of every
environment variable (name, default, matching `InitOptions` field, precedence) is the `InitOptions`
field docs in `src/init.rs`; `README.md` keeps a short quick-reference table, and
`tests/docs_consistency_test.rs` fails when that table drifts from the `DEFAULT_*` constants.

When adding, removing or re-defaulting an environment variable, update in this order:

1. the resolver and its `DEFAULT_*` constant (`src/file_tracing.rs`, `src/timestamp.rs`; crate-wide
   constants such as `DEFAULT_LOG_LEVEL` live in `src/lib.rs`);
2. the matching `InitOptions` field doc in `src/init.rs` — this is the authoritative copy;
3. the quick-reference table in `README.md` **and** `SUPPORTED_VARS` in
   `tests/docs_consistency_test.rs`; the test fails when either is missed.

`.env` is loaded via `dotenvy` with `dotenv_override()`, so it takes precedence over the process
environment. `InitOptions` fields override the corresponding environment variables.

**Every variable is read with a prefix** (`InitOptions::env_prefix` + the documented name), because
this is a public crate: `LOG_DIR` and `APP_MODE` are generic enough that an unrelated program may
already own them. Build the name with `crate::env_var_name(env_prefix, "LOG_DIR")` — never
`std::env::var("LOG_DIR")` — and keep `RUST_LOG` unprefixed, since the Rust logging ecosystem
shares that name. `InitOptions::new` rejects an empty prefix, so the unprefixed names cannot come
back by accident.

`.env` is loaded **before** the application mode is resolved: the mode precedence is
`InitOptions::mode_override` → mode env var (the prefixed `APP_MODE` by default, `.env` included) →
`Production`. Keep that order when touching `init()` — resolving the mode before loading `.env`
would ignore an `APP_MODE` defined in the file. An explicit `InitOptions::mode_env_var` is used
verbatim, without the prefix.
