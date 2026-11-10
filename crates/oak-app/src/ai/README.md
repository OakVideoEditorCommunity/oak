# `crates/oak-app/src/ai` — built-in agent backend (T8.1)

The sovereign's 2026-11-11 ruling: **all in-process**. The chat panel and
the LLM tool loop live inside the oak process — no OPP plugin, no pi. This
module is the backend half; the panel UI comes in a later task.

## Layout

| file | responsibility |
| --- | --- |
| `config.rs` | `OAK_AI_API_KEY` / `OAK_AI_BASE_URL` / `OAK_AI_MODEL`; missing key ⇒ structured `ConfigError` (`AI_NOT_CONFIGURED`), never a panic. `Debug` redacts the key. |
| `llm.rs` | openai-completions client behind the `LlmClient` trait (the mock seam): request/response types, `tool_calls` parsing, structured `LlmError`, the frozen-registry → `tools` adapter, and the blocking `UreqClient` (ureq 3, already a dependency). |
| `agent.rs` | the loop: system + history + `dispatch::default_schemas()` → LLM → `tool_calls` → `dispatch::execute_with` (hooks transaction gate, unchanged) → results appended as `tool` messages → repeat until no `tool_calls` or `max_rounds`. Reports `converged` vs `capped` honestly. |
| `backend.rs` | `MockBackend` (tests) and `HostAdapter<H: HostServices>` — the lead ruled scheme B (2026-11-11): a thin JSON↔struct adapter over the **`HostServices` trait**, never the concrete `OppHost`, so the OPP implementation stays replaceable and coupling lives at the assembly point. |

## Configuration

```bash
export OAK_AI_API_KEY=...        # required; never written to disk/log/report
export OAK_AI_BASE_URL=https://token-plan-cn.xiaomimimo.com/v1   # default
export OAK_AI_MODEL=mimo-v2.6-flash                             # default
```

## Relationship to `ai/oak-ai-tools`

This module **depends on** `ai/oak-ai-tools` unchanged: the frozen 22-tool
schemas (`dispatch::default_schemas`), the dispatcher
(`dispatch::execute_with`), the hooks policy/gate (`hooks::*`) and the
transaction primitives all stay there. Nothing in `ai/` was edited for
T8.1. The only new surface is orchestration (this directory).

## Tests

`cargo test --offline -p oak-app ai::` (needs the local `cc` shim on PATH
for linking) runs 13 tests, none of them touching the network:

* `config` — missing/blank key is structured; defaults fill in; `Debug`
  redacts the key.
* `llm` — response parsing (tool calls, malformed arguments, missing
  `choices`); 22-entry `tools` array from the frozen registry.
* `agent` — the six brief scenarios: single tool round, multi-round
  dependency (`project.open` → `project.get_overview`), no-tool-call
  convergence, round-cap stop, missing-key structured error, and the hooks
  gate rejection (engine left clean, reason handed to the model).

## Known limitations

* **No streaming** — one blocking `chat/completions` call per turn
  (v1; the panel shows turns as they finish).
* **`HostAdapter` field mapping is mock-untested** — the adapter compiles
  and the mapping table is derived from the frozen schemas and
  `host_services.rs` definitions, but no real-engine run has exercised it
  yet. Two tools return structured `UNSUPPORTED` rather than guess across
  a semantic gap: `render.get_audio_levels` (`{sequence,range,resolution}`
  vs host `{clip,buckets}`) and `render.get_thumbnails` (`range` vs
  `clips`). Schema parameters the host signature does not carry
  (`project.open` `fps`/`name`, `timeline.add_track` `sequence`/`index`,
  …) are dropped exactly as the existing OPP dispatch path drops them.
* **No live-network test** — `UreqClient` is exercised only in the real
  run, not in CI (key discipline: tests never read the environment).
* **Root workspace line** — building `oak-app` against
  `ai/oak-ai-tools` required one lead-approved line in the root
  `Cargo.toml` (`exclude += "ai/oak-ai-tools"`); `ai/` itself is
  untouched.
