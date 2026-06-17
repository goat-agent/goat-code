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

36 crates organized into six layers, with `goat-protocol` at the bottom of the dependency DAG:

**Infrastructure**
- `goat-protocol` — shared wire contract (`Op`, `Event`, `TaskId`); serde only; leaf.
- `goat-config` — config, `ThemeChoice`, `~/.goat-code` paths, log directory; no TUI deps; leaf.
- `goat-core` — `Session` and the `Engine` trait; depends on `goat-protocol` only.
- `goat-tui` — full-screen ratatui app (The Elm Architecture); depends on `goat-protocol`, `goat-commands`, and `goat-config`, not `goat-core` or any engine crate.
- `goat-code` — the `goat` binary; wires the CLI, logging, and `goat daemon` subcommands; runs as a thin client that connects to (or auto-spawns) the daemon.
- `goat-wire` — daemon/client wire contract; leaf (depends on `goat-protocol` only). The `ClientFrame`/`ServerFrame` envelope ({`SessionId`/`ClientId`/`seq` + payload `Op`/`Event`}), length-delimited JSON codec (`WireConn`), and protocol-version handshake. `Op`/`Event` bodies are wrapped, never modified.
- `goat-daemon` — the resident `goatd` (`goat daemon serve`); machine-wide single daemon holding N live sessions keyed by cwd. Owns the session registry, a single seq-stamping event-log pump per session (stamp→log→fan-out), per-window bounded delivery with disconnect-on-overflow, presence broadcast, idle eviction (kept alive while a turn runs or a window is attached or an Ask/Plan is open), orphaned-turn sweep on startup, and the unix-socket listener (`~/.goat-code/daemon.sock`, 0600). Allocates per-session `TaskId`s and echoes a correlation token.
- `goat-client` — thin transport the TUI talks to; auto-spawns the daemon if absent, performs the handshake, opens/reattaches a session, and exposes the same `Op`/`Event` channels the TUI already consumes. Owns the bidirectional `IdMap` (client-local ↔ daemon `TaskId`) and seq-gap resync.
- `goat-update` — executable replacement helper for `goat update`; small CLI-only crate with no app-state ownership.
- `goat-worktree` — git-worktree management (`enter`/`list`/`remove`); `enter` resolves and returns the worktree path (the agent cwd is injected explicitly, not via process `set_current_dir` for the engine).

**Providers**
- `goat-provider` — the `Provider` trait; leaf. Key types: `Provider`, `Request` (incl. `ToolChoice`), `StreamEvent`, `StreamError`, `Message`, `Capabilities`, `Model`, `ProviderId`, `ContentBlock`. Providers classify their own wire errors into `StreamError` structurally (`error.rs` per provider); the engine never inspects error strings.
- `goat-provider-anthropic` — Anthropic Claude API provider; per-model context windows, prompt-caching `cache_control` breakpoints (tools + system + last two messages), `stop_reason` overflow detection.
- `goat-provider-gemini` — Google Gemini provider; API key (Generative Language API) or OAuth (Code Assist free tier, gemini-cli compatible); four modules: `lib` (provider orchestration), `wire` (Gemini request/response format), `oauth` (Google OAuth PKCE flow), `codeassist` (Code Assist envelope + project onboarding).
- `goat-provider-openai-compat` — OpenAI-family HTTP clients; three modules: `chat` (Chat Completions API, used by local providers), `responses` (Responses API, used by OpenAI and Codex), `common` (shared client/validate/discovery helpers).
- `goat-provider-openai` — OpenAI provider (wraps `responses` module).
- `goat-provider-openai-codex` — OpenAI Codex provider (wraps `responses` module).
- `goat-provider-local` — table-driven local-inference provider (Ollama, LM Studio, llama.cpp); wraps `chat` module.
- `goat-providers` — provider registry; wires all provider crates. `Registry::new(store)` for default account, `Registry::load(store, account)` for explicit. `Registry::login(provider, status)` dispatches OAuth login through the `Provider::login` trait method.

**Agent**
- `goat-agent` — `GoatAgent`, the production `Engine` implementation; owns the LLM loop, tool dispatch, and the `Conversation` history (messages + db row ids). Long-running capabilities live here: retry with exponential backoff over classified provider errors (`retry.rs`), mid-turn steering (queued `SubmitMessage` injected at round boundaries), and LLM-summarization auto-compaction with a `ContextTracker` token budget (`compaction.rs`). Also owns the `Agent` delegation tool and `AgentSpec`/`AgentRegistry` (`agent.rs`): built-in `explore` (read-only) and `general`, plus file-defined agents from `.goat/agents/<name>.md` (Claude Code custom-agent frontmatter — `name`/`description`/`tools`/`model`/`effort`). Module map and dependency direction live in `crates/goat-agent/AGENTS.md`.
- `goat-mcp` — MCP (Model Context Protocol) client manager; launches stdio MCP servers and adapts their tools into the `Tool` trait via `rmcp`. A `goat-agent` dependency.
- `goat-sandbox` — platform sandbox backend for shell execution (deny-file rules); used by `goat-tool-shell` and `goat-agent`.

**Auth / Store**
- `goat-auth` — credential store (provider API keys, OAuth tokens).
- `goat-store` — conversation persistence (SQLite via rusqlite).

**Tools**
- `goat-tool` — the `Tool` trait, `ToolOutput` (model-facing content + optional human summary), and per-tool input display (`display_input`, generic fallback in `display`); depends on `goat-protocol` only.
- `goat-tool-fs` — filesystem tools (read, write, list, search).
- `goat-tool-shell` — shell execution tool.
- `goat-tool-search` — web/code search tools.
- `goat-tool-web` — the web-fetch tool; fetches a URL over HTTPS and converts to Markdown, with SSRF protection (`ssrf` module rejects private/link-local addresses).
- `goat-tool-skill` — the `Skill` tool; loads a skill's instructions on demand from the cwd.
- `goat-tool-computer` — the `Computer` tool; desktop control (screenshot + mouse/keyboard) via `xcap`/`enigo`. Opt-in: registered by `GoatAgent::new` only when `computer_use_enabled` is set.
- `goat-tool-browser` — the `Browser` tool; drives a real Chrome via CDP (`chromiumoxide`). One tool with an `action` enum (navigate/snapshot/click/type/select/press_key/evaluate/screenshot/close); actions return a text accessibility snapshot with element refs (`screenshot` returns an image). Persistent login profile at `~/.goat-code/browser/profile`, headful. Opt-in: registered by `GoatAgent::new` only when `browser_enabled` is set.
- `goat-tools` — tool registry; wires all tool crates.
- `goat-skill` — SKILL.md (agentskills.io) parser and loader; reads global `~/.goat-code/skills` and project `.goat/skills` (project overrides global); depends on `goat-config` only.

**Commands**
- `goat-command` — the `Command` trait (`&'static str` name/description, `run → CommandEffect`) and `CommandEffect`/`CommandSpec`; leaf, mirrors `goat-tool`.
- `goat-command-settings` — `/model`, `/effort`, `/config` commands (one module per command). `/model` and `/effort` accept an optional argument (`/model <name>`, `/effort <level>`) or open a picker when bare.
- `goat-command-conversation` — `/clear`, `/compact`, and `/resume` commands. `/compact [focus]` summarizes the conversation to free context (deferred to after the turn when one is running). `/resume` opens a picker of past conversations in the cwd, or `/resume <n>` resumes the nth.
- `goat-command-help` — `/help` command.
- `goat-command-app` — app-lifecycle commands (`/exit`).
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
- `GoatAgent` owns a `Conversation` history (single source of truth for the LLM context); the TUI keeps an append-only render mirror built from `Event`s. Each message is persisted losslessly as a `Vec<ContentBlock>` JSON `body` (thinking blocks and tool calls/results included); the messages table is append-only. Compactions are recorded in a separate `compactions` table (summary + tail/preserved boundaries), so `/resume` rebuilds the compacted engine history while the transcript replays the full scrollback with compaction markers.
- Long-running policy is split by ownership: providers classify wire failures into `StreamError` variants; the engine owns what to do with each — `RateLimited`/`Overloaded`/`Transport` retry with jittered backoff (honoring `retry_after`), `ContextOverflow` triggers reactive compaction then retries the round once, `Auth`/`InvalidRequest`/`Other` abort the turn. Proactive compaction fires between rounds when the `ContextTracker` estimate crosses `window − reserve`; the summarization call reuses the session's exact tools/system with `ToolChoice::None` so tool use is structurally impossible.
- Mid-turn `Op::SubmitMessage` is steering: it queues in the turn's `SteeringQueue` and injects as a user message at the next round boundary (`Event::UserMessage` confirms placement); a turn ends only when the model stops and the queue is empty. `Op::DequeueMessage` retracts a queued message (`Event::MessageDequeued` confirms); whichever event arrives is the truth the TUI renders.
- Reasoning effort is a per-model property carried on `ModelTarget.effort` (persisted per thread). Providers advertise the valid set per model via `Provider::efforts` and translate the chosen `Request.effort` themselves — OpenAI/Codex send `reasoning.effort`, Anthropic maps to `output_config.effort`/`thinking.budget_tokens`. Anthropic extended thinking requires the `ContentBlock::Thinking`/`RedactedThinking` blocks to round-trip unchanged in history, which is why they are first-class content blocks every provider must handle.
- The `Agent` tool is engine-level, not a registry tool: the model calls it like a tool, but `GoatAgent` intercepts the call in dispatch and runs the same loop core nested — its own history, restricted tool set (no `Agent`, so no recursion), a child `TaskId`, and no persistence. Several run concurrently via a semaphore-bounded `join_all`, and a parent `CancellationToken` fans out to every child on interrupt. The shared loop core is parameterized by a `Run` (top-level emits + persists; child emits child-tagged events only).
- Human-facing tool presentation belongs to the tools, never the TUI: each tool renders its own input via `Tool::display_input` (parsing its own `Input` struct) and may attach a display `summary` to `ToolOutput`; the engine ships both over `goat-protocol` (`ToolCall.display`, `ToolOutcome.summary`), and the TUI renders exactly what arrives with zero per-tool knowledge. Screenshot-producing tools (computer/browser) ship the captured image too: the engine attaches it to `ToolOutcome.image` (alongside the provider-history `ContentBlock::Image`) and the TUI renders it inline via `ratatui-image`, using the terminal's graphics protocol (Kitty/iTerm2/Sixel) or unicode-halfblock fallback. The image is live-session only — it is not persisted, so `/resume` shows the text marker.
- The TUI normalizes three event sources into one `AppEvent`, runs a pure `App::update` reducer, and renders on a dirty flag — never on every tick. Child-agent events are routed by `TaskId` to a per-run transcript; a footer agent selector (↓ to focus, arrows, Esc to leave) drills the main area into one run by swapping which transcript renders — the same swap mechanism `/resume` uses.
- The composer is a first-party widget. Do not add `tui-textarea`; it does not support ratatui 0.30.
- On startup, `GoatAgent` reads project `AGENTS.md` files and injects them into the system prompt. Discovery follows the Codex standard: global `~/.goat-code/AGENTS.md` first, then git root → cwd (root-to-leaf order, each file capped at 32 KiB). `AGENTS.override.md` in any directory takes precedence over `AGENTS.md` in the same directory. The same injected content reaches both the main loop and delegated subagents.

## Distribution

Native install only. The root `install.sh` is the official Unix/macOS installer and installs
`goat` plus `goat-update` into `/usr/local/bin`; Windows initial install is archive-only.
GitHub Actions builds stable release archives and `SHA256SUMS`; `goat update` stages releases
and `goat-update` performs executable replacement. Installation metadata is not persisted; the
install location is fixed by platform policy. `cargo-release` owns version bumping and
`v{{version}}` tag creation; pushed release tags trigger `.github/workflows/release.yml`.
The project is not published to crates.io and internal crates are `publish = false`. Do not add
`cargo install` distribution flows and do not reintroduce cargo-dist.

## Testing

The full-screen TUI needs a real tty, so it is not driven headlessly. Test the pure
`App::update` reducer and the engine's `Op → Event` behavior instead. The binary's non-TUI
paths (`--version`, `--help`, `update`, `--print-log-path`) are safe to run anywhere.

When a crate grows conventions of its own, add a nested `crates/<name>/AGENTS.md`; the
closest file wins.
