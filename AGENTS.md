# Repository Guidelines

## Project Structure & Module Organization
- `src/main.rs` wires the application, loads environment variables, and launches the TUI.
- `src/agent.rs` contains the core agent loop and orchestration.
- `src/ai_sdk/` hosts provider integrations (currently Anthropic in `src/ai_sdk/anthropic.rs`).
- `src/tools/` defines tool interfaces and implementations (`read_file`, `list_files`, `edit_file`).
- `src/ui.rs` renders the terminal UI using Ratatui.

## Build, Test, and Development Commands
- `cargo run` runs the TUI locally; requires `ANTHROPIC_API_KEY` in the environment.
- `cargo build` compiles the project for local development.
- `cargo test` runs Rust tests (no suite yet, but use this once tests are added).

## Coding Style & Naming Conventions
- Follow Rust defaults: 4-space indentation, `snake_case` for functions/modules, `CamelCase` for types.
- Prefer small, focused modules that mirror file names (e.g., `agent.rs` -> `agent` module).
- Format with `cargo fmt` and keep lint warnings minimal (use `cargo clippy` before PRs if possible).

## Testing Guidelines
- No automated tests are currently checked in.
- Add unit tests in the same module with `#[cfg(test)] mod tests` and integration tests under `tests/`.
- Keep test names descriptive and action-oriented (e.g., `handles_empty_prompt`).

## Commit & Pull Request Guidelines
- Commit messages are short and imperative (e.g., "fix residual input line bug", "bump crossterm").
- Keep commits specific; avoid vague messages like "update" when possible.
- PRs should include: a concise summary, test steps (or note "not run"), and any config changes.

## Configuration & Secrets
- The app expects `ANTHROPIC_API_KEY`; load via shell env or a local `.env` file.
- Do not commit API keys or local configuration files.
