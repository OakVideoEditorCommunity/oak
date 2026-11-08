#!/usr/bin/env bash
# oak-ai MCP 服务器启动包装（pi 的 stdio MCP 客户端只 exec 一个可执行文件+参数，
# 不做 shell 展开，所以「定位二进制 → 选后端」这一步必须落在包装脚本里）。
#
# 解析顺序：
#   1. $OAK_AI_MCP_BIN（绝对路径，脚本不猜）
#   2. <repo>/ai/oak-ai-mcp/target/debug/oak-ai-mcp
#   3. <repo>/ai/oak-ai-mcp/target/release/oak-ai-mcp
# 后端：$OAK_AI_MCP_BACKEND（默认 mock；sdk 需该二进制以 --features sdk 构建）
#
# 本脚本只做定位，不触发构建——构建由 scripts/build-mcp.sh 显式执行，避免 agent
# 启动时把 6 核机器拖进引擎构建。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"

backend="${OAK_AI_MCP_BACKEND:-mock}"
bin="$(oak_mcp_bin || true)"

if [ -z "$bin" ]; then
	echo "oak-mcp: 找不到 oak-ai-mcp 二进制。先跑 ai/agent/scripts/build-mcp.sh" >&2
	echo "oak-mcp: 已查找 \$OAK_AI_MCP_BIN 与 ai/oak-ai-mcp/target/{debug,release}/oak-ai-mcp" >&2
	exit 2
fi

# 注意：stdout 是 MCP 协议通道，包装层的任何诊断只能走 stderr。
case "$backend" in
mock | sdk) ;;
*)
	echo "oak-mcp: 未知后端 '$backend'（只接受 mock|sdk）" >&2
	exit 2
	;;
esac

exec "$bin" --backend "$backend"
