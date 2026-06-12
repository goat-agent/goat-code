# goat-agent

`GoatAgent` is the production `Engine`. The crate is split by responsibility; `lib.rs` is the
shared-types hub (`GoatAgent`, `run()` op loop, `Ctx`, `Run`/`Report`/`TurnIds`, `LoopEnv`,
`Flow`) and every module imports from it.

## Modules

| Module | Owns |
|---|---|
| `prompt` | `SYSTEM_PROMPT`, system-prompt assembly, skill listing |
| `accounts` | login/account lifecycle, model discovery, per-account registries, `provider_for` |
| `threads` | thread listing/rename/resume, stored-message parsing |
| `persist` | every goat-store write: threads, turns, messages, tool calls, `now_ms` |
| `turn` | `handle_turn` (top-level turn lifecycle, mid-turn op select loop), `handle_idle_op`, `TurnEnd` |
| `rounds` | `core_loop`, `run_round` (provider stream consumption), `process_round_output` |
| `tools_exec` | tool defs, parallel tool batches, `execute_tool` routing, display helpers |
| `delegate` | the `Agent` tool: spec resolution, child runs, concurrency cap |
| `ask` | the `Ask` tool: question schema, blocking answer channel |
| `agent` | `AgentSpec`/`AgentRegistry` (built-ins + `.goat/agents/*.md`) |
| `instructions` | AGENTS.md discovery and injection |
| `rate_limit_cache` | rate-limit snapshot persistence |

## Dependency direction

`turn → rounds`; `rounds → tools_exec → {delegate, ask}`; `delegate → rounds::core_loop` is the
one intentional back-edge (the delegation recursion itself, `Box::pin`ned). `turn`/`threads`/
`accounts` lean on `persist`; `accounts` and `threads` are otherwise leaves. Engine integration
tests live in `lib.rs`; unit tests sit next to what they exercise.
