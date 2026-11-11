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
| `memory.rs` | `memory.*` — long-term memory over the **memlite** CLI (BM25, two tiers). Subprocess isolation, Rust-side secret refusal, structured errors. See the section below. |
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

## `memory.*` — long-term memory (T11A)

The sovereign's instruction (2026-11-11): integrate memlite, **BM25 only**
(no embedding service, no respire). The CLI is a separate binary, so
`memory.rs` wraps it as a subprocess and adds the three policies the CLI does
not provide.

### Tools (a third face in the loop; the frozen 22 and the v2 face are untouched)

| tool | arguments | returns |
| --- | --- | --- |
| `memory.remember` | `content` (required), `title`, `tags` (comma-separated), `project` (scope override) | `{id (12 hex), tier, title, records, embedded}` |
| `memory.recall` | `query` (required), `limit` (1–20, default **5**) | `{query, backend:"bm25", hits:[{id,title,score,tags,tier,content≤400 chars}], searched:[…], fallback?}` |
| `memory.show` | `id` (required) | the full entry, `content` capped at 4000 chars + `truncated` |
| `memory.list` | `limit` (1–100, default **20**) | `{entries:[{id,title,tags,ts,tier}], tiers, limit}` |
| `memory.forget` | `id` (required) | `{deleted, tier}` |
| `memory.status` | — | `{bin, project, tiers:[{tier,dir,records,backend}], records, backend:"bm25"}` |

Recall returns at most five hits and truncates each body to 400 characters —
a memory is a hint, not a document; `memory.show` is the "read it properly"
tool. Nothing is injected into the prompt automatically (lead's ruling): the
loop only advertises the tools in the system message and lets the model ask.

### Where the libraries live

Two tiers, both under the **application data directory** (`$OAK_HOME`, i.e.
`~/.oak` by default) — never inside the `.oak` project file, which must stay
a document and not a cache:

```
~/.oak/memory/memlite.json          # Oak's private CLI config (no embedding block)
~/.oak/memory/<project-id>-<hash8>/ # project tier (memory.jsonl + .git)
~/.oak/memory/user/                 # user tier (the fallback)
```

`project-id` comes from `OAK_MEMORY_PROJECT` or the assembly point that
knows the library row uuid; a path hash is only the fallback because it
breaks when the project moves. The directory name keeps the printable part
of the id plus an FNV-1a suffix, so two ids that sanitise alike stay distinct.

### Why subprocess, and the two measured traps

1. **`--data-dir` alone is not isolation.** With only the flag, `recall`
   still merges `~/.memlite/config.json` and dials its embedding endpoint
   (`connect(… htons(18321))`, measured with strace) before falling back to
   BM25. Every call therefore sets `MEMLITE_CONFIG` to Oak's own config **and**
   passes `--data-dir`, and `--bm25` pins the lexical path. The user's
   `~/.memlite/config.json` is never read for writes and never written.
2. **The CLI's own secret gate has holes.** memlite v0.1.0 (2026-10-14)
   rejects an 18-digit id and an 11-digit phone, but stored
   `sk-proj-…` and `数据库口令 = …` with rc=0 (measured). The refusal
   therefore lives in Rust (`find_secret`), before the child is spawned, and
   covers `sk-*`/`sk-proj-*`, `ghp_/gho_/github_pat_`, `AKIA…`, PEM private
   key blocks, `Authorization`/`Bearer`, `api_key=`/`token=`/`password=`/
   `口令=`/`密码=`, 18-digit id cards and 11-digit phones.

### Error vocabulary

CLI failures are `rc=1` + plain stderr (there is no JSON error shape even
under `--json`), and "nothing found" is `rc=0` + empty hits — so the two are
kept apart here: a missing library directory or an unknown id is a structured
`NOT_FOUND` with a next step (`memory.list`), an empty query or empty content
is `INVALID_PARAMS`, a missing binary is `UNSUPPORTED` with gap id
`oak.memory.memlite-binary-missing` plus an install hint, and a CLI overrun
(10 s) is `STATE_ERROR` — never a panic and never a silent success.

### Not in the undo stack, on purpose

`memory.*` does **not** open an edit transaction and does not call
`group_begin`: it writes memory-library files, not the project document, so
there is nothing for `oak-undo` to restore. This also keeps it clear of the
OPP nesting trap (an `edit_begin` group is already open in that path, and a
second `group_begin` inside it answers `Error::State`). The dispatch branch
in `agent.rs` routes `memory.*` **before** the v2 and frozen-22 paths for
exactly this reason; if a future change makes memory writes part of the
engine document, that branch must be revisited.

## Tests

`cargo test --offline -p oak-app ai::` (needs the local `cc` shim on PATH
for linking) runs the backend tests plus the `memory` suite; none of them
touches the network:

* `config` — missing/blank key is structured; defaults fill in; `Debug`
  redacts the key.
* `llm` — response parsing (tool calls, malformed arguments, missing
  `choices`); 22-entry `tools` array from the frozen registry.
* `agent` — the six brief scenarios: single tool round, multi-round
  dependency (`project.open` → `project.get_overview`), no-tool-call
  convergence, round-cap stop, missing-key structured error, and the hooks
  gate rejection (engine left clean, reason handed to the model).
* `memory` — secret-gate table (eight patterns refused, six ordinary facts
  not); id/path derivation; missing binary ⇒ `UNSUPPORTED`+gap; empty
  query/content ⇒ `INVALID_PARAMS`; a real-CLI round trip in a temp library
  (remember → recall → show → list → forget → `NOT_FOUND`); two-tier
  fallback; private-config isolation (`~/.memlite/config.json` byte-identical
  before/after); four concurrent remembers with no lost records; and a slow
  fake CLI that must trip the timeout as a structured error.

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
