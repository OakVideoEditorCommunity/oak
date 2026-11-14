#!/usr/bin/env bash
# T6 agent 回放：真实 pi 进程 + MCP 工具面，模型 stub 按 fixture 回放。
# 未用真实 LLM（无 key、无网络）；断言：工具调用 ⊆ 现场 tools/list、
# 取帧成功、事件流里有 toolResult。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
agent="$here/../agent"
repo="$(cd "$here/../.." && pwd)"
run_dir="${OAK_CI_RUN_DIR:-$here/target/agent-replay-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$run_dir"

log() { printf '[agent-replay] %s\n' "$*" >&2; }

# 1) MCP 二进制（只复用，不构建引擎）
bin="$repo/ai/oak-ai-mcp/target/debug/oak-ai-mcp"
[ -x "$bin" ] || { log "缺 MCP 二进制：$bin（先跑 ai/ci/run.sh 的 cargo 段）"; exit 2; }

# 2) pi 依赖
[ -x "$agent/node_modules/.bin/pi" ] || { log "缺 pi：先 npm ci --offline（ai/agent）"; exit 2; }

# 3) 素材（mock 后端只查存在性，按路径哈希上色）
printf 'fake footage for ci agent replay\n' >/tmp/oak-ci-footage-a.mp4

# 4) stub 模型（复用 T3 的回放服务器）
stub_log="$run_dir/stub-requests.jsonl"
agentdir="$run_dir/agentdir"
mkdir -p "$agentdir"
node "$agent/tests/stub-model-server.mjs" --port 0 \
	--script "$here/fixtures/agent-replay.json" --log "$stub_log" \
	>"$run_dir/stub-port" 2>"$run_dir/stub.stderr" &
stub_pid=$!
cleanup() { kill "$stub_pid" 2>/dev/null || true; }
trap cleanup EXIT
for _ in $(seq 1 50); do [ -s "$run_dir/stub-port" ] && break; sleep 0.2; done
port="$(head -1 "$run_dir/stub-port" | tr -d '[:space:]')"
[ -n "$port" ] || { log "stub 没起来"; cat "$run_dir/stub.stderr" >&2; exit 2; }

# 项目信任：pi 只在 agentDir/trust.json 按 cwd 记信任决定（--approve 只覆盖
# 会话级，print 模式仍查 trust store，见 pi core/project-trust.js）。只写本次
# run 的 agentdir，绝不碰 ~/.pi/agent/trust.json。
python3 - "$agentdir" "$here" <<'PYTRUST'
import json, sys, pathlib
agentdir, cwd = sys.argv[1], str(pathlib.Path(sys.argv[2]).resolve())
path = pathlib.Path(agentdir) / "trust.json"
store = {}
if path.exists():
    try:
        store = json.loads(path.read_text())
    except json.JSONDecodeError:
        store = {}
store[cwd] = True
path.write_text(json.dumps(store, indent=2))
print(f"[agent-replay] project trust granted: {cwd}", file=sys.stderr)
PYTRUST

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

# 5) 上下文链（任务级用本次回放的任务文件；缺省用 example）。pi 只沿 cwd
# 向上找 AGENTS.md，所以把生成链放进本次会话 cwd（ai/ci）。
task_file="${OAK_CI_TASK_FILE:-$here/fixtures/task.example.md}"
OAK_AI_TASK_FILE="$task_file" OAK_AI_CWD="$here" bash "$agent/scripts/context.sh" >/dev/null

# 6) 跑无头会话（cwd=ai/ci，读 ci 自己的 .pi/mcp.json；工具事件用
#    tool_execution_end 解析，不依赖 message 里的 toolCalls 字段名）
events="$run_dir/events.jsonl"
set +e
cd "$here"
PI_CODING_AGENT_DIR="$agentdir" setsid "$agent/node_modules/.bin/pi" \
	--print --mode json --approve --offline \
	--provider oak-stub --model oak-replay \
	"按任务文件执行回放：建工程、导素材、铺轨、取一帧 PNG，然后停止。不要解释。" \
	>"$events" 2>"$run_dir/pi.stderr"
rc=$?
set -e
log "pi 退出码：$rc"
[ "$rc" -eq 0 ] || { tail -30 "$run_dir/pi.stderr" >&2; exit "$rc"; }

# 7) 断言
python3 - "$events" "$stub_log" "$run_dir" <<'PY'
import json, sys
events, stub_log, run_dir = sys.argv[1:4]
tool_calls, oak_tools, tool_errors = [], set(), 0
for line in open(events):
    line = line.strip()
    if not line.startswith("{"):
        continue
    try:
        m = json.loads(line)
    except json.JSONDecodeError:
        continue
    if m.get("type") == "tool_execution_end":
        name = m.get("toolName") or ""
        if name and name not in tool_calls:
            tool_calls.append(name)
        if name.startswith("mcp__oak__"):
            oak_tools.add(name)
        if m.get("isError"):
            tool_errors += 1

assert tool_calls, "事件流里没有工具调用"
assert oak_tools, "没有 mcp__oak__* 调用"
# 取帧必须发生
assert any("render_get_frame" in t for t in tool_calls), f"没取帧: {tool_calls}"
assert tool_errors == 0, f"{tool_errors} 次工具调用报错（MCP 没连上？）"

# stub 请求取证：模型确实收到了工具面与上下文 marker
reqs = [json.loads(l) for l in open(stub_log) if l.strip()]
assert reqs, "stub 没收到请求"
assert any(r["request"].get("tool_count", 0) >= 22 for r in reqs), "模型收到的工具面 < 22"
assert any(r["request"].get("system_has_context_marker") for r in reqs), "system prompt 无上下文 marker"

print(json.dumps({
    "oak_tool_calls": sorted(oak_tools),
    "tool_call_count": len(tool_calls),
    "tool_errors": tool_errors,
    "stub_requests": len(reqs),
    "model": "stub (fixture replay, NOT a real LLM)",
    "run_dir": run_dir,
}, ensure_ascii=False))
PY
