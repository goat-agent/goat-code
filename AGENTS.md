# AGENTS.md — goat-code

goat-code is a Rust terminal coding-agent CLI rendered as a full-screen TUI. This file is
the single source of truth for agents working in this repo; `CLAUDE.md` imports it.

## Commands

| Command | Purpose |
|---------|---------|
| `cargo build --workspace` | Build every crate |
| `cargo run` | Launch the TUI (needs a real terminal / tty) |
| `cargo test --workspace` | Run all tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lint; warnings are errors |
| `cargo fmt --all` | Format (`--check` to verify only) |

Before calling any change done, `cargo fmt --all`, the `clippy` line above, and
`cargo test --workspace` must all pass.

## Workspace

27 crates organized into six layers, with `goat-protocol` at the bottom of the dependency DAG:

**Infrastructure**
- `goat-protocol` — shared wire contract (`Op`, `Event`, `TaskId`); serde only; leaf.
- `goat-config` — config, `ThemeChoice`, XDG paths, log directory; no TUI deps; leaf.
- `goat-core` — `Session` and the `Engine` trait; depends on `goat-protocol` only.
- `goat-tui` — full-screen ratatui app (The Elm Architecture); depends on `goat-protocol` and `goat-commands`, not `goat-core` or any engine crate.
- `goat-code` — the `goat` binary; wires the channels, logging, and CLI; depends on all.

**Providers**
- `goat-provider` — the `Provider` trait; leaf. Key types: `Provider`, `Request`, `StreamEvent`, `Message`, `Capabilities`, `Model`, `ProviderId`, `ContentBlock`.
- `goat-provider-anthropic` — Anthropic Claude API provider.
- `goat-provider-openai-compat` — OpenAI-family HTTP clients; three modules: `chat` (Chat Completions API, used by local providers), `responses` (Responses API, used by OpenAI and Codex), `common` (shared client/validate/discovery helpers).
- `goat-provider-openai` — OpenAI provider (wraps `responses` module).
- `goat-provider-openai-codex` — OpenAI Codex provider (wraps `responses` module).
- `goat-provider-local` — table-driven local-inference provider (Ollama, LM Studio, llama.cpp); wraps `chat` module.
- `goat-providers` — provider registry; wires all provider crates. `Registry::new(store)` for default account, `Registry::load(store, account)` for explicit. `Registry::login(provider, status)` dispatches OAuth login through the `Provider::login` trait method.

**Agent**
- `goat-agent` — `GoatAgent`, the production `Engine` implementation; owns the LLM loop, tool dispatch, and `Vec<Message>` history. Also owns the `Agent` delegation tool and `AgentSpec`/`AgentRegistry` (`agent.rs`): built-in `explore` (read-only) and `general`, plus file-defined agents from `.goat/agents/<name>.md` (Claude Code custom-agent frontmatter — `name`/`description`/`tools`/`model`/`effort`). Project instruction loading lives in `instructions.rs`.

**Auth / Store**
- `goat-auth` — credential store (provider API keys, OAuth tokens).
- `goat-store` — conversation persistence (SQLite via rusqlite).

**Tools**
- `goat-tool` — the `Tool` trait; leaf.
- `goat-tool-fs` — filesystem tools (read, write, list, search).
- `goat-tool-shell` — shell execution tool.
- `goat-tool-search` — web/code search tools.
- `goat-tool-skill` — the `Skill` tool; loads a skill's instructions on demand from the cwd.
- `goat-tools` — tool registry; wires all tool crates.
- `goat-skill` — SKILL.md (agentskills.io) parser and loader; reads global `~/.goat-code/skills` and project `.goat/skills` (project overrides global); depends on `goat-config` only.

**Commands**
- `goat-command` — the `Command` trait (`&'static str` name/description, `run → CommandEffect`) and `CommandEffect`/`CommandSpec`; leaf, mirrors `goat-tool`.
- `goat-command-settings` — `/model`, `/effort`, `/config` commands (one module per command). `/model` and `/effort` accept an optional argument (`/model <name>`, `/effort <level>`) or open a picker when bare.
- `goat-command-conversation` — `/clear` and `/resume` commands. `/resume` opens a picker of past conversations in the cwd, or `/resume <n>` resumes the nth.
- `goat-command-help` — `/help` command.
- `goat-commands` — command registry; wires the per-category command crates and surfaces loaded skills as `/name` commands via `set_skills`; mirrors `goat-tools`.

The UI and the engine communicate only through `goat-protocol` types over bounded
`tokio::mpsc` channels. The binary owns both channels and connects them.

## Rules

- **No comments.** Write none of any kind — no `//`, `///`, `//!`, block comments, or TOML `#`. Convey intent through names and structure.
- `unsafe` is forbidden workspace-wide (`unsafe_code = "forbid"`). clippy `pedantic` runs at warn; keep the tree clean under `-D warnings`.
- Edition 2024, MSRV 1.95; `rust-toolchain.toml` tracks `stable` (let-chains and `cfg_select` rely on a current compiler).
- Errors: library crates use `thiserror` enums; the application boundary uses `color_eyre::Result`.
- **Logging goes to a rolling file, never stdout/stderr** — stdout corrupts the full-screen TUI. Use `tracing`; `GOAT_LOG` sets the filter and `goat --print-log-path` prints the directory.
- Centralize dependency versions in the root `[workspace.dependencies]`; crates inherit with `{ workspace = true }`.

## Architecture

- `goat-core` stays feature-free forever: it owns the session lifecycle and the `Op → Event` loop and nothing else. Real capability (LLM, tools) plugs in above core by implementing the `Engine` trait. `GoatAgent` is the production engine.
- `Engine` is an object-safe actor: `fn spawn(self, ops, events) -> JoinHandle`. No `async_trait`, no `Stream`.
- `GoatAgent` owns a `Vec<Message>` history (single source of truth for the LLM context); the TUI keeps an append-only render mirror built from `Event`s. Each message is persisted losslessly as a `Vec<ContentBlock>` JSON `body` (thinking blocks and tool calls/results included), so `/resume` rebuilds both the history and the transcript from the store.
- Reasoning effort is a per-model property carried on `ModelTarget.effort` (persisted per thread). Providers advertise the valid set per model via `Provider::efforts` and translate the chosen `Request.effort` themselves — OpenAI/Codex send `reasoning.effort`, Anthropic maps to `output_config.effort`/`thinking.budget_tokens`. Anthropic extended thinking requires the `ContentBlock::Thinking`/`RedactedThinking` blocks to round-trip unchanged in history, which is why they are first-class content blocks every provider must handle.
- The `Agent` tool is engine-level, not a registry tool: the model calls it like a tool, but `GoatAgent` intercepts the call in dispatch and runs the same loop core nested — its own history, restricted tool set (no `Agent`, so no recursion), a child `TaskId`, and no persistence. Several run concurrently via a semaphore-bounded `join_all`, and a parent `CancellationToken` fans out to every child on interrupt. The shared loop core is parameterized by a `Run` (top-level emits + persists; child emits child-tagged events only).
- The TUI normalizes three event sources into one `AppEvent`, runs a pure `App::update` reducer, and renders on a dirty flag — never on every tick. Child-agent events are routed by `TaskId` to a per-run transcript; a footer agent selector (↓ to focus, arrows, Esc to leave) drills the main area into one run by swapping which transcript renders — the same swap mechanism `/resume` uses.
- The composer is a first-party widget. Do not add `tui-textarea`; it does not support ratatui 0.30.
- On startup, `GoatAgent` reads project `AGENTS.md` files and injects them into the system prompt. Discovery follows the Codex standard: global `~/.goat-code/AGENTS.md` first, then git root → cwd (root-to-leaf order, each file capped at 32 KiB). `AGENTS.override.md` in any directory takes precedence over `AGENTS.md` in the same directory. The same injected content reaches both the main loop and delegated subagents.

## Distribution

Native install only, via `cargo-dist` (install.sh + GitHub Releases + axoupdater). The
project is not published to crates.io and internal crates are `publish = false`; the binary
opts into distribution with `[package.metadata.dist] dist = true`. Do not add `cargo install`
flows or crates.io publishing.

A release is cut by pushing a tag: `git tag vX.Y.Z && git push --tags`. The release workflow
`.github/workflows/release.yml` is generated by `dist generate` from `dist-workspace.toml` —
change the config and regenerate; never hand-edit the workflow.

## Testing

The full-screen TUI needs a real tty, so it is not driven headlessly. Test the pure
`App::update` reducer and the engine's `Op → Event` behavior instead. The binary's non-TUI
paths (`--version`, `--help`, `update`, `--print-log-path`) are safe to run anywhere.

When a crate grows conventions of its own, add a nested `crates/<name>/AGENTS.md`; the
closest file wins.
