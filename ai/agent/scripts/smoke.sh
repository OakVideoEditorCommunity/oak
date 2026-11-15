#!/usr/bin/env bash
# 无头冒烟：pi 非交互跑一次真实 agent 循环（上下文链 + MCP 工具调用 + 结果回喂）。
#
# 模型侧默认用本地 stub（tests/stub-model-server.mjs 回放 fixture）——**没调真实
# LLM**，只验证「链路真的通了」。要接真实模型：OAK_AI_SMOKE_MODEL=provider/model
# 且导出对应 key 环境变量，脚本会跳过 stub、直接用该模型。
#
# 校验（全部断言，任一失败退出非 0）：
#   1. pi 退出码 0，事件流里出现 MCP 工具调用与 toolResult；
#   2. 上下文链三层 marker + 记忆层 marker 都进了 system prompt；
#   3. 模型请求里的工具名集合 ⊇ 现场 tools/list 给出的 22 个工具（无本地副本）；
#   4. 会话里调用的每个 mcp__oak__* 都在现场 tools/list 里（防幻觉/防漂移）。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../scripts" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"

agent_dir="$(oak_agent_dir)"
tests_dir="$agent_dir/tests"
run_root="${OAK_AI_HOME}/runs"
run_id="smoke-$(date +%Y%m%d-%H%M%S)"
run_dir="$run_root/$run_id"
mkdir -p "$run_dir"

log() { printf '[smoke] %s\n' "$*" >&2; }

# 1) 二进制与上下文
bin="$(bash "$here/build-mcp.sh")"
log "MCP 二进制：$bin"
bash "$here/context.sh" >/dev/null
log "上下文链已生成：$agent_dir/AGENTS.md"

# 2) 现场取工具面（唯一真相源）
tools_json="$run_dir/tools-list.json"
python3 - "$bin" "$tools_json" <<'PY'
import json, subprocess, sys
bin_path, out_path = sys.argv[1], sys.argv[2]
frames = [
    {"jsonrpc": "2.0", "id": 1, "method": "initialize",
     "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": {"name": "oak-smoke", "version": "1"}}},
    {"jsonrpc": "2.0", "method": "notifications/initialized"},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
]
p = subprocess.run([bin_path, "--backend", "mock"],
                   input="".join(json.dumps(f) + "\n" for f in frames),
                   capture_output=True, text=True, timeout=120)
for line in p.stdout.splitlines():
    line = line.strip()
    if not line.startswith("{"):
        continue
    msg = json.loads(line)
    if msg.get("id") == 2:
        tools = msg["result"]["tools"]
        json.dump([t["name"] for t in tools], open(out_path, "w"))
        print("\n".join(t["name"] for t in tools))
        sys.exit(0)
print("smoke: 现场 tools/list 失败", file=sys.stderr)
sys.exit(2)
PY
mapfile -t live_tools < <(python3 -c 'import json,sys;print("\n".join(json.load(open(sys.argv[1]))))' "$tools_json")
[ "${#live_tools[@]}" -gt 0 ] || { log "tools/list 为空"; exit 2; }
log "现场 tools/list：${#live_tools[@]} 个工具"

# 3) 模型侧：stub（默认）或真实模型
stub_pid=""
stub_log="$run_dir/stub-requests.jsonl"
agentdir="$run_dir/agentdir"
mkdir -p "$agentdir"

if [ -n "${OAK_AI_SMOKE_MODEL:-}" ]; then
	model_args=(--model "$OAK_AI_SMOKE_MODEL")
	PI_CODING_AGENT_DIR="${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}"
	log "模型：$OAK_AI_SMOKE_MODEL（真实 LLM；未标注为 mock）"
else
	node "$tests_dir/stub-model-server.mjs" --port 0 \
		--script "$tests_dir/fixtures/smoke.json" --log "$stub_log" \
		>"$run_dir/stub-port" 2>"$run_dir/stub.stderr" &
	stub_pid=$!
	for _ in $(seq 1 50); do [ -s "$run_dir/stub-port" ] && break; sleep 0.2; done
	port="$(head -1 "$run_dir/stub-port" | tr -d '[:space:]')"
	[ -n "$port" ] || { log "stub 模型服务没起来"; cat "$run_dir/stub.stderr" >&2; exit 2; }
	cat >"$agentdir/models.json" <<JSON
{
	"providers": {
		"oak-stub": {
			"baseUrl": "http://127.0.0.1:${port}/v1",
			"api": "openai-completions",
			"apiKey": "oak-local-stub-not-a-secret",
			"models": [{ "id": "oak-replay", "name": "oak-replay (fixture 回放，非真实 LLM)", "input": ["text"], "output": ["text"] }]
		}
	}
}
JSON
	PI_CODING_AGENT_DIR="$agentdir"
	model_args=(--provider oak-stub --model oak-replay)
	log "模型：本地 stub（fixture 回放，未用真实 LLM）"
fi

cleanup() {
	if [ -n "$stub_pid" ]; then kill "$stub_pid" 2>/dev/null || true; fi
}
trap cleanup EXIT

# 4) 跑一次无头会话。--approve：信任仓内 .pi/mcp.json（MCP 配置在仓内，单文件可审）
events="$run_dir/events.jsonl"
set +e
PI_CODING_AGENT_DIR="$PI_CODING_AGENT_DIR" setsid "$agent_dir/node_modules/.bin/pi" \
	--print --mode json --approve --offline "${model_args[@]}" \
	"冒烟：建一个工程、加一条视频轨并读总览；不需要解释，直接调用工具。" \
	>"$events" 2>"$run_dir/pi.stderr"
rc=$?
set -e
log "pi 退出码：$rc"
[ "$rc" -eq 0 ] || { tail -20 "$run_dir/pi.stderr" >&2; exit "$rc"; }

# 5) 断言
python3 - "$events" "$stub_log" "$tools_json" "$run_dir" <<'PY'
import json, sys, pathlib
events_path, stub_log, tools_path, run_dir = sys.argv[1:5]
events = [json.loads(l) for l in pathlib.Path(events_path).read_text().splitlines() if l.strip()]
live = set(json.load(open(tools_path)))
fails = []

# 5.1 上下文链 marker
sysmsg = next((e["message"] for e in events if e.get("type") == "message_start" and e["message"].get("role") == "system"), None)
if not sysmsg:
    fails.append("事件流里没有 system 消息")
else:
    ctx = sysmsg["sections"].get("project_context", "")
    for marker, label in [
        ("OAK-AGENT-CONTEXT-BEGIN", "上下文链头"), ("OAK-AGENT-CONTEXT-END", "上下文链尾"),
        ("layer:1/4 repo", "1 仓级"), ("layer:2/4 project", "2 项目级"),
        ("layer:3/4 task", "3 任务级"), ("layer:4/4 memory", "4 记忆层"),
    ]:
        if marker not in ctx:
            fails.append(f"上下文链缺 {label}（{marker}）")
    if "第 1 层 · 仓级上下文" not in ctx:
        fails.append("仓级层正文没进上下文")

# 5.2 工具调用与结果（事件流里同一消息会在 message_end / turn_end 各出现一次，按 id 去重）
calls, results = {}, {}
for e in events:
    m = e.get("message")
    if not m:
        continue
    content = m.get("content")
    for c in content if isinstance(content, list) else []:
        if isinstance(c, dict) and c.get("type") == "toolCall":
            calls[c.get("id")] = c
    if m.get("role") == "toolResult":
        results[m.get("toolCallId")] = m
calls = list(calls.values())
results = list(results.values())
if not calls:
    fails.append("没有任何工具调用")
oak_calls = [c for c in calls if c.get("name", "").startswith("mcp__oak__")]
if not oak_calls:
    fails.append("没有 mcp__oak__* 工具调用（MCP 未接通）")
for c in oak_calls:
    if c["name"] not in {f"mcp__oak__{n.replace('.', '_').replace('-', '_')}" for n in live}:
        fails.append(f"会话调用了不在现场 tools/list 里的工具：{c['name']}")
if not any(r.get("isError") is not True for r in results):
    fails.append("所有工具结果都是失败")

# 5.3 模型请求侧：工具面来自 MCP，且无本地副本
reqs = [json.loads(l) for l in pathlib.Path(stub_log).read_text().splitlines() if l.strip()] if pathlib.Path(stub_log).exists() else []
if reqs:
    req_first = reqs[0]["request"]
    mcp_names = {n for n in req_first["tool_names"] if n.startswith("mcp__oak__")}
    if len(req_first["tool_names"]) and not mcp_names:
        fails.append("模型请求里没有 MCP 工具")
    if not req_first.get("system_has_context_marker"):
        fails.append("模型请求的 system 里没有上下文链 marker")

summary = {
    "session_events": len(events),
    "tool_calls": len(calls),
    "oak_tool_calls": [c["name"] for c in oak_calls],
    "tool_results": len(results),
    "live_tool_count": len(live),
    "model": "stub (fixture replay, NOT a real LLM)" if reqs else "real/other",
    "fails": fails,
}
pathlib.Path(run_dir, "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2))
print(json.dumps(summary, ensure_ascii=False, indent=2))
sys.exit(1 if fails else 0)
PY

# 最终文本（人看的）
python3 - "$events" <<'PY'
import json, sys, pathlib
events = [json.loads(l) for l in pathlib.Path(sys.argv[1]).read_text().splitlines() if l.strip()]
texts = []
for e in events:
    m = e.get("message") or {}
    if m.get("role") == "assistant" and isinstance(m.get("content"), list):
        for c in m["content"]:
            if isinstance(c, dict) and c.get("type") == "text":
                texts.append(c["text"])
print("[smoke] 最终回复：" + (texts[-1] if texts else "(无)"))
PY
log "产物目录：$run_dir"
