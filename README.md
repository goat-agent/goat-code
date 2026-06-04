# goat-code

A terminal coding agent, in Rust — full-screen TUI, clean architecture, native install.

> Early scaffolding. The UI and the engine seam are in place; the agent is a stub that
> streams a canned reply. Real model and tool execution plug in behind the `Engine` trait
> without touching the UI.

## Install

Once the first release is published, install the single static binary (no runtime needed):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/jbj338033/goat-code/releases/latest/download/goat-code-installer.sh | sh
```

Then upgrade in place with `goat update`.

## Build from source

```sh
git clone https://github.com/jbj338033/goat-code
cd goat-code
cargo run
```

Requires Rust 1.86+ (the toolchain is pinned to 1.93). `cargo run` launches the TUI and
needs a real terminal. Enter sends, Shift/Opt+Enter inserts a newline, Ctrl+C interrupts a
running task or quits.

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Conventions and architecture live in [AGENTS.md](AGENTS.md).

## Layout

| Crate | Responsibility |
|-------|----------------|
| `goat-protocol` | Shared `Op` / `Event` contract |
| `goat-config` | Config and theme selection |
| `goat-core` | Session runtime and the `Engine` seam |
| `goat-tui` | The full-screen ratatui UI |
| `goat-code` | The `goat` binary |

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
