#!/usr/bin/env bash
# 交互启动：定位/构建 MCP 二进制 → 生成三层上下文链 → 拉起 pi（cwd=ai/agent）。
#
# 用法：
#   scripts/start.sh                          # 用 ~/.pi/agent 里的模型配置
#   scripts/start.sh --model <provider/model> # 指定模型
#   OAK_AI_AGENT_DIR=/path scripts/start.sh   # 换 agent-dir（隔离配置/多套 key）
#   OAK_AI_MCP_BACKEND=sdk scripts/start.sh   # 真引擎后端（需 sdk 构建产物）
#
# 密钥：pi 从环境变量或 agent-dir 的 auth.json 取，不在任何仓内文件里。仓内
# `.pi/mcp.json` 只描述「怎么起 MCP server」，不含凭据。`--approve` 用于信任这份
# 仓内 MCP 配置（它会被 pi 当作启动脚本执行，审过再用）。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"

agent_dir="$(oak_agent_dir)"
bin="$(bash "$here/build-mcp.sh")" || exit 2
printf '[start] MCP: %s (backend=%s)\n' "$bin" "${OAK_AI_MCP_BACKEND:-mock}" >&2

bash "$here/context.sh" >&2

export PI_CODING_AGENT_DIR="${OAK_AI_AGENT_DIR:-${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}}"
printf '[start] agent-dir: %s\n' "$PI_CODING_AGENT_DIR" >&2
printf '[start] 记忆库: 工程=%s 用户=%s\n' "$(oak_project_mem)" "$(oak_user_mem)" >&2

cd "$agent_dir"
exec ./node_modules/.bin/pi --approve "$@"
