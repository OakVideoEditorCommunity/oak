#!/usr/bin/env bash
# 共用路径与派生值。被 scripts/*.sh source，不单独执行。
#
# 目录约定（全部仓外，公开仓零泄漏）：
#   OAK_AI_HOME   = ~/.local/share/oak-ai
#   工程级记忆库   = $OAK_AI_HOME/projects/<project-id>/
#   用户级兜底库   = $OAK_AI_HOME/user/
#   私有 memlite 配置 = $OAK_AI_HOME/config/memlite.json（绝不碰 ~/.memlite）
#   会话/冒烟产物  = $OAK_AI_HOME/runs/<timestamp>/
# project-id 由仓库路径 sha256 前缀派生（可被 $OAK_AI_PROJECT_ID 覆盖），
# 不含本机路径明文，可安全出现在仓内文档示例里。

set -euo pipefail

: "${OAK_AI_HOME:=$HOME/.local/share/oak-ai}"

# memlite 的 init 会把 data_dir 写回配置文件；默认配置是 ~/.memlite/config.json
# ——用户的全局配置，本 agent 不得改动（2026-11-05 实测会覆盖其 data_dir）。
# 所以把配置路径钉到仓外私有文件，并让它默认指向用户级兜底库：agent 范围内任何
# memlite 调用（含 memlite 自带的 user fallback）都落在 oak-ai 自己的库。
export MEMLITE_CONFIG="${OAK_AI_HOME}/config/memlite.json"

oak_agent_dir() { cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd; }

oak_repo_root() {
	local d
	d="$(oak_agent_dir)"
	git -C "$d" rev-parse --show-toplevel 2>/dev/null || printf '%s\n' "$d"
}

# 工程 id：优先环境变量；否则 "<仓名>-<abs 路径 sha256 前 8 位>"
oak_project_id() {
	if [ -n "${OAK_AI_PROJECT_ID:-}" ]; then
		printf '%s\n' "$OAK_AI_PROJECT_ID"
		return
	fi
	local root name hash
	root="$(oak_repo_root)"
	name="$(basename "$root")"
	hash="$(printf '%s' "$root" | sha256sum | cut -c1-8)"
	printf '%s-%s\n' "$name" "$hash"
}

oak_project_mem() { printf '%s/projects/%s\n' "$OAK_AI_HOME" "$(oak_project_id)"; }
oak_user_mem() { printf '%s/user\n' "$OAK_AI_HOME"; }

# 让私有配置的 data_dir 指向用户级兜底库（memlite 未命中工程库时会用它）。
oak_mem_sync_config() {
	mkdir -p "$(dirname "$MEMLITE_CONFIG")"
	MEMLITE_CFG_PATH="$MEMLITE_CONFIG" OAK_MEM_USER_DIR="$(oak_user_mem)" python3 - <<'PY'
import json, os
path = os.environ["MEMLITE_CFG_PATH"]
user_dir = os.environ["OAK_MEM_USER_DIR"]
data = {}
if os.path.exists(path):
    try:
        data = json.load(open(path))
    except Exception:
        data = {}
data["data_dir"] = user_dir
json.dump(data, open(path, "w"), ensure_ascii=False, indent=2)
PY
}

oak_memlite() {
	if [ -n "${OAK_AI_MEMLITE:-}" ]; then
		printf '%s\n' "$OAK_AI_MEMLITE"
		return
	fi
	local c
	c="$(command -v memlite || true)"
	if [ -z "$c" ] && [ -x "$HOME/.local/bin/memlite" ]; then
		c="$HOME/.local/bin/memlite"
	fi
	[ -n "$c" ] || {
		echo "oak-mem: 找不到 memlite（$HOME/.local/bin/memlite）" >&2
		return 2
	}
	printf '%s\n' "$c"
}

oak_mcp_bin() {
	if [ -n "${OAK_AI_MCP_BIN:-}" ] && [ -x "${OAK_AI_MCP_BIN}" ]; then
		printf '%s\n' "${OAK_AI_MCP_BIN}"
		return
	fi
	local root cand
	root="$(oak_repo_root)"
	for cand in \
		"$root/ai/oak-ai-mcp/target/debug/oak-ai-mcp" \
		"$root/ai/oak-ai-mcp/target/release/oak-ai-mcp"; do
		if [ -x "$cand" ]; then
			printf '%s\n' "$cand"
			return
		fi
	done
	return 1
}

# 幂等初始化：memlite init 会建 git 仓并重写配置，所以只在缺 .git 时做；做完把
# 私有配置里的 data_dir 复位到用户级兜底库。
oak_ensure_dirs() {
	local d mem
	mem="$(oak_memlite)"
	for d in "$(oak_project_mem)" "$(oak_user_mem)"; do
		if [ ! -d "$d/.git" ]; then
			mkdir -p "$d"
			"$mem" --data-dir "$d" init >/dev/null
		fi
	done
	oak_mem_sync_config
}
