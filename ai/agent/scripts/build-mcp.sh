#!/usr/bin/env bash
# 构建 / 定位 oak-ai-mcp 二进制。默认 features 只依赖 oak-ai-tools（无引擎、
# 无 FFmpeg），因此不会把机器拖进引擎构建；已存在的二进制直接复用。
#
# 用法：
#   scripts/build-mcp.sh            # 有就复用，没有就默认 features 构建
#   scripts/build-mcp.sh --rebuild  # 强制 cargo build（仍为默认 features）
#   OAK_AI_MCP_BACKEND=sdk scripts/build-mcp.sh --sdk   # 需引擎，明确要求才做
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"

root="$(oak_repo_root)"
crate="$root/ai/oak-ai-mcp"
rebuild=0
sdk=0
for a in "$@"; do
	case "$a" in
	--rebuild) rebuild=1 ;;
	--sdk) sdk=1 ;;
	-h | --help)
		sed -n '2,12p' "${BASH_SOURCE[0]}"
		exit 0
		;;
	*)
		echo "build-mcp: 未知参数 $a" >&2
		exit 2
		;;
	esac
done

if [ -n "${OAK_AI_MCP_BIN:-}" ] && [ -x "${OAK_AI_MCP_BIN}" ] && [ "$rebuild" -eq 0 ]; then
	echo "$OAK_AI_MCP_BIN"
	exit 0
fi

if [ "$rebuild" -eq 0 ]; then
	if bin="$(oak_mcp_bin)"; then
		echo "$bin"
		exit 0
	fi
fi

[ -d "$crate" ] || {
	echo "build-mcp: 缺少 $crate（T5 交付物）" >&2
	exit 2
}

echo "build-mcp: 在 $crate 构建（默认 features，不拉引擎）" >&2
if [ "$sdk" -eq 1 ]; then
	echo "build-mcp: --sdk → 需要引擎依赖链，可能耗时数十分钟；本机多线并行时请确认资源" >&2
	(cd "$crate" && cargo build --features sdk)
else
	(cd "$crate" && cargo build)
fi

oak_mcp_bin
