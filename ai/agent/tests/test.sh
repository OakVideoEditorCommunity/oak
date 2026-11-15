#!/usr/bin/env bash
# ai/agent 自测：三个验收面各跑一遍真实链路，全部可重复、无 key、无网络。
#
#   A 无头冒烟        scripts/smoke.sh（pi --print 走 MCP 22 工具，模型用 fixture 回放）
#   B 跨会话记忆      会话1 经 pi 的 bash 工具写经验 → 会话2 经 pi 的 bash 工具召回命中
#   C 密钥不落盘      敏感闸拒写 + 全树 grep 证明无 key 正文
#   D 上下文注入      检索词命中时记忆层进 AGENTS.md 链（再进 system prompt）
#
# 用法：tests/test.sh [--keep]   默认用一次性临时 $OAK_AI_HOME，不碰真实记忆库。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../scripts" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"

agent_dir="$(oak_agent_dir)"
tests_dir="$agent_dir/tests"
keep=0
[ "${1:-}" = "--keep" ] && keep=1

work="$(mktemp -d "${TMPDIR:-/tmp}/oak-agent-test.XXXXXX")"
export OAK_AI_HOME="$work/home"
mkdir -p "$OAK_AI_HOME"
ok=0
fail=0

cleanup() {
	if [ "$keep" -eq 1 ]; then
		printf '[test] 保留产物：%s\n' "$work"
	else
		rm -rf "$work"
	fi
}
trap cleanup EXIT

pass() { ok=$((ok + 1)); printf '[PASS] %s\n' "$*"; }
die() { fail=$((fail + 1)); printf '[FAIL] %s\n' "$*"; }

# 起一个 stub 模型服务；$1=fixture $2=日志 $3=输出目录；结果放 $STUB_PID / $STUB_PORT
# （不用命令替换取端口：那会把 $! 关进子 shell，后面就 kill 不到了）
STUB_PID=""
STUB_PORT=""
start_stub() {
	local fixture="$1" log="$2" outdir="$3"
	node "$tests_dir/stub-model-server.mjs" --port 0 --script "$fixture" --log "$log" \
		>"$outdir/port" 2>"$outdir/stub.stderr" &
	STUB_PID=$!
	for _ in $(seq 1 50); do [ -s "$outdir/port" ] && break; sleep 0.2; done
	STUB_PORT="$(head -1 "$outdir/port" | tr -d '[:space:]')"
}

write_models() { # $1=agentdir $2=port
	mkdir -p "$1"
	cat >"$1/models.json" <<JSON
{
	"providers": {
		"oak-stub": {
			"baseUrl": "http://127.0.0.1:$2/v1",
			"api": "openai-completions",
			"apiKey": "oak-local-stub-not-a-secret",
			"models": [{ "id": "oak-replay", "input": ["text"], "output": ["text"] }]
		}
	}
}
JSON
}

run_session() { # $1=fixture $2=workdir $3=prompt $4=events out
	local fixture="$1" wd="$2" prompt="$3" events="$4"
	mkdir -p "$wd"
	start_stub "$fixture" "$wd/stub.jsonl" "$wd"
	[ -n "$STUB_PORT" ] || { echo "stub 未启动" >&2; return 2; }
	write_models "$wd/agentdir" "$STUB_PORT"
	PI_CODING_AGENT_DIR="$wd/agentdir" setsid timeout 120 \
		"$agent_dir/node_modules/.bin/pi" --print --mode json --approve --offline \
		--provider oak-stub --model oak-replay "$prompt" >"$events" 2>"$wd/pi.stderr"
	local rc=$?
	kill "$STUB_PID" 2>/dev/null || true
	return $rc
}

# ---------- A 无头冒烟 ----------
printf '\n===== A 无头冒烟（pi --print + MCP 22 工具，模型为 fixture 回放）=====\n'
if OAK_AI_HOME="$work/home" bash "$here/smoke.sh" >"$work/smoke.log" 2>&1; then
	pass "A 冒烟：$(python3 - "$work" <<'PY'
import json, glob, sys
runs = sorted(glob.glob(sys.argv[1] + "/home/runs/smoke-*"))
s = json.load(open(runs[-1] + "/summary.json"))
print("tool_calls=%d oak_calls=%d live_tools=%d fails=%d" % (
    s["tool_calls"], len(s["oak_tool_calls"]), s["live_tool_count"], len(s["fails"])))
PY
)"
else
	die "A 冒烟失败，日志：$work/smoke.log"
	tail -20 "$work/smoke.log" || true
fi

# ---------- D 记忆注入上下文链 ----------
printf '\n===== D 记忆召回注入上下文链 =====\n'
cat >"$work/task.md" <<'TASK'
# 任务级（测试）
- 任务目标：验证记忆层注入
- 记忆检索词：硬切点 prescan
TASK
"$here/oak-mem.sh" remember --title "测试经验：硬切点先跑 prescan 峰值表" \
	"粗剪双机位时，硬切点先跑 prescan 峰值表再取帧确认。" >"$work/mem-write.json"
OAK_AI_TASK_FILE="$work/task.md" bash "$here/context.sh" --print >"$work/AGENTS.preview.md"
if grep -q "测试经验：硬切点先跑 prescan 峰值表" "$work/AGENTS.preview.md" &&
	grep -q "layer:4/4 memory" "$work/AGENTS.preview.md"; then
	pass "D 记忆命中已注入 layer 4（$(grep -c '^- \*\*' "$work/AGENTS.preview.md") 条）"
else
	die "D 记忆层未注入，见 $work/AGENTS.preview.md"
fi

# ---------- B 跨会话记忆 ----------
printf '\n===== B 跨会话记忆（会话1 写 → 会话2 召回）=====\n'
# 会话夹具用仓内文件（tests/fixtures/memory-*.json），与真实跑法一致
cp "$tests_dir/fixtures/memory-write.json" "$work/s1.json"
cp "$tests_dir/fixtures/memory-recall.json" "$work/s2.json"
( cd "$agent_dir" && run_session "$work/s1.json" "$work/s1" "记下这条粗剪经验" "$work/s1/events.jsonl" ) || die "B1 会话1 执行失败"
sid="$(python3 - "$work/s1/events.jsonl" <<'PY'
import json, re, sys, pathlib
p = pathlib.Path(sys.argv[1])
ids = set()
if p.exists():
    for line in p.read_text().splitlines():
        if not line.strip():
            continue
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        m = e.get("message") or {}
        content = m.get("content")
        for c in content if isinstance(content, list) else []:
            if isinstance(c, dict):
                for mm in re.finditer(r'"id":\s*"([0-9a-f]{8,})"', c.get("text", "") or ""):
                    ids.add(mm.group(1))
print(sorted(ids)[0] if ids else "NO_ID")
PY
)"
if [ -n "$sid" ] && [ "$sid" != "NO_ID" ]; then
	pass "B1 会话1 通过 pi 的 bash 工具写入成功，记录 id=$sid"
else
	die "B1 会话1 未写出记录（sid=$sid）"
fi

recall_out="$work/s2/events.jsonl"
( cd "$agent_dir" && run_session "$work/s2.json" "$work/s2" "按上次的剪辑经验继续" "$recall_out" ) || true
if [ -f "$recall_out" ] && grep -q "$sid" "$recall_out"; then
	pass "B2 会话2 召回到会话1 的同一条记录（id=$sid 出现在 bash 工具结果里）"
else
	die "B2 会话2 未召回 $sid"
fi

# ---------- C 密钥不落盘 ----------
printf '\n===== C 密钥不落盘 =====\n'
# 假 key 由 printf 拼出：文件里不出现完整的 "sk-<20+ 字符>" 字面量，免得公开仓的
# 泄漏扫描误报；运行时仍是完整假值，C2 用它做「正文没有写进去」的实测。
fake_key="sk-$(printf '%s' test-0123456789abcdef0123456789abcdef)"
set +e
"$here/oak-mem.sh" remember --title "不该写进去的经验" "把 key $fake_key 记下来" >"$work/gate.out" 2>"$work/gate.err"
gate_rc=$?
set -e
if [ "$gate_rc" -eq 3 ]; then
	pass "C1 敏感闸拒绝写入（exit=3）：$(head -1 "$work/gate.err")"
else
	die "C1 敏感闸没有拒绝（exit=$gate_rc）"
fi
if grep -rq "$fake_key" "$OAK_AI_HOME/projects" "$OAK_AI_HOME/user" 2>/dev/null; then
	die "C2 假 key 正文出现在记忆库里"
else
	pass "C2 记忆库正文无 key（grep -r 未命中 $fake_key 前缀 sk-test-）"
fi
# 仓库内将提交的文件 + 运行产物都扫一遍（不含 node_modules）
hits="$(grep -rInE 'sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|BEGIN [A-Z ]*PRIVATE KEY' \
	"$agent_dir" --exclude-dir=node_modules --exclude-dir=.agentdir --exclude="*.oak-agent-bak-*" 2>/dev/null |
	grep -v "$fake_key" || true)"
if [ -z "$hits" ]; then
	pass "C3 ai/agent 全树（去 node_modules）无 key 形态正文"
else
	die "C3 发现疑似 key 形态：$hits"
fi
if grep -q '"apiKey": "oak-local-stub-not-a-secret"' "$agent_dir/tests"/*.mjs 2>/dev/null ||
	grep -rq 'oak-local-stub-not-a-secret' "$agent_dir/scripts" "$agent_dir/tests"; then
	pass "C4 模型配置里只有明显的假值 oak-local-stub-not-a-secret"
else
	die "C4 未见假值占位（配置可能改过）"
fi
if [ -f "$HOME/.memlite/config.json" ]; then
	if python3 -c "
import json,sys
d=json.load(open('$HOME/.memlite/config.json'))
sys.exit(0 if 'oak-ai' not in str(d.get('data_dir','')) else 1)"; then
		pass "C5 用户级 ~/.memlite/config.json 未被本 agent 改写（data_dir 不含 oak-ai）"
	else
		die "C5 用户级 memlite 配置被改到 oak-ai 目录"
	fi
fi

# ---------- 汇总 ----------
printf '\n===== 汇总：%d 通过，%d 失败 =====\n' "$ok" "$fail"
if [ "$fail" -ne 0 ]; then
	[ "$keep" -eq 0 ] && printf '[test] 失败现场已删除，重跑请加 --keep\n'
	exit 1
fi
