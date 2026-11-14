#!/usr/bin/env bash
# T6 无头 CI 总入口：一条命令跑完 ai/ 全链（schema → prescan → 执行层 → MCP →
# agent 回放），全部无引擎、无 key、无网络（cargo --offline / pi --offline）。
#
# 用法：
#   ai/ci/run.sh                 # 全绿基线
#   OAK_CI_SKIP_AGENT=1 ai/ci/run.sh   # 跳过 pi agent 回放（缺 node_modules 时）
#   OAK_CI_SKIP_PY=1 ai/ci/run.sh      # 跳过 python 段（无 ffmpeg 的机器）
#
# 每段独立记结果，任一红最后退出非 0；末尾给总计。
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
cd "$repo"

declare -a names results
fail=0

# log_dir 缺省仓外（公开仓零泄漏）；--locked 要求 Cargo.lock 与 manifest 一致。
log_dir="${OAK_CI_LOG_DIR:-${HOME}/.local/share/oak-ai/runs/ci-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$log_dir"
echo "[run] 日志目录：$log_dir"

step() {
	local name="$1"
	shift
	local log="$log_dir/${name}.log"
	printf '\n=== [%s] %s ===\n' "$name" "$*"
	if "$@" >"$log" 2>&1; then
		names+=("$name"); results+=("PASS")
	else
		names+=("$name"); results+=("FAIL")
		fail=1
		echo "--- $name 失败，末 40 行：$log ---"
		tail -40 "$log"
	fi
}

# 1) T0 schema
if [ "${OAK_CI_SKIP_PY:-0}" != 1 ]; then
	step "schema" python3 ai/schema/test_schema.py
	# 2) T2 prescan（要 ffmpeg）
	step "prescan" python3 ai/prescan/test_prescan.py
else
	echo "[run] OAK_CI_SKIP_PY=1：跳过 schema/prescan"
fi

# 3) T1 执行层（默认 features，无引擎）
step "oak-ai-tools" cargo test --offline --locked --manifest-path ai/oak-ai-tools/Cargo.toml

# 4) T5 MCP 协议（无引擎）
step "oak-ai-mcp" cargo test --offline --locked --manifest-path ai/oak-ai-mcp/Cargo.toml

# 5) T6 回放库 + 黄金帧 + 零漂移
step "ci-replay" cargo test --offline --locked --manifest-path ai/ci/Cargo.toml

# 6) 真实 pi 进程 agent 回放（stub 模型，未用真实 LLM）
if [ "${OAK_CI_SKIP_AGENT:-0}" != 1 ]; then
	step "agent-replay" bash ai/ci/scripts/agent-replay.sh
else
	echo "[run] OAK_CI_SKIP_AGENT=1：跳过 agent 回放"
fi

# 7) workflow 静态检查：actionlint 优先，缺则 PyYAML 解析 + 结构断言
step "workflow-lint" bash ai/ci/scripts/lint-workflows.sh

printf '\n=== 汇总 ===\n'
for i in "${!names[@]}"; do
	printf '%-16s %s\n' "${names[$i]}" "${results[$i]}"
done
if [ "$fail" -ne 0 ]; then
	echo "RESULT: FAIL"
	exit 1
fi
echo "RESULT: OK"
exit 0
