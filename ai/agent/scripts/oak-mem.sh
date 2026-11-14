#!/usr/bin/env bash
# oak agent 记忆层：memlite 两档库的唯一入口。
#
# 两档：
#   工程级  $OAK_AI_HOME/projects/<project-id>/   默认写入/默认检索（仓外）
#   用户级  $OAK_AI_HOME/user/                    工程库检索为空时的兜底
# 两档目录都由本脚本按需 init；--data-dir 永远显式传，不依赖 memlite 全局配置。
#
# 敏感闸：正文命中密钥/token/口令/私钥形态即**拒写**（退出码 3），提示改存位置
# 指针。闸在写入前跑，不是事后扫描；这是纪律的可执行形式，不靠模型自觉。
#
# 用法：
#   scripts/oak-mem.sh remember --title "事实句标题" [--tags a,b] [--stdin] <正文>
#   scripts/oak-mem.sh recall <查询词> [--limit N] [--json|--format md] [--no-fallback]
#   scripts/oak-mem.sh show <id>
#   scripts/oak-mem.sh list [--limit N]
#   scripts/oak-mem.sh status
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"

cmd="${1:-}"
[ -n "$cmd" ] || {
	sed -n '2,20p' "${BASH_SOURCE[0]}"
	exit 2
}
shift || true

# 密钥形态检测：宁可误拒（提示改存指针）也不放过正文。

if ! command -v python3 >/dev/null 2>&1; then
	echo "oak-mem: 需要 python3 做敏感闸正则匹配与 JSON 整形" >&2
	exit 2
fi

secret_re_py() { # $1 = 正文；命中则不打印、退出 3
	python3 - "$1" <<'PY' || exit 3
import re, sys
text = sys.argv[1]
pats = [
    r"sk-[A-Za-z0-9_-]{16,}",
    r"(?i)(api[_-]?key|apikey)\s*[:=：]",
    r"(?i)(token|secret|passwd|password)\s*[:=：]",
    r"(密钥|口令|密码)\s*[:=：]",
    r"(?i)Bearer\s+[A-Za-z0-9._~+/-]{12,}",
    r"-----?BEGIN [A-Z ]*PRIVATE KEY",
    r"AKIA[0-9A-Z]{16}",
    r"gh[pousr]_[A-Za-z0-9]{20,}",
    r"xox[baprs]-[A-Za-z0-9-]{10,}",
]
hits = [p for p in pats if re.search(p, text)]
if hits:
    print("oak-mem: 拒绝写入——正文疑似含密钥/token/口令（命中 %d 条规则）。" % len(hits), file=sys.stderr)
    print("oak-mem: 记忆正文只存位置指针，例如「模型 key 在 <文件> 的 <字段>」，不要存正文。", file=sys.stderr)
    sys.exit(3)
PY
}

# 幂等初始化：memlite init 会顺手建 git 仓并重写私有配置，所以只在缺 .git 时做，
# 调用方随后用 oak_mem_sync_config 把配置的 data_dir 复位到用户级兜底库。
ensure_db() {
	local d="$1"
	if [ ! -d "$d/.git" ]; then
		mkdir -p "$d"
		"$(oak_memlite)" --data-dir "$d" init >/dev/null
	fi
}

case "$cmd" in
remember)
	title=""
	tags=""
	use_stdin=0
	content=""
	while [ $# -gt 0 ]; do
		case "$1" in
		--title) title="${2:-}"; shift 2 ;;
		--tags) tags="${2:-}"; shift 2 ;;
		--stdin) use_stdin=1; shift ;;
		-h | --help) sed -n '2,20p' "${BASH_SOURCE[0]}"; exit 0 ;;
		*) content="$1"; shift ;;
		esac
	done
	if [ "$use_stdin" -eq 1 ]; then
		content="$(cat)"
	fi
	[ -n "$content" ] || { echo "oak-mem: remember 需要正文（位置参数或 --stdin）" >&2; exit 2; }
	secret_re_py "$content"
	dir="$(oak_project_mem)"
	ensure_db "$dir"
	args=(--title "$title")
	[ -n "$tags" ] && args+=(--tags "$tags")
	args+=(--project "$(oak_project_id)")
	[ -z "$title" ] && args=(--project "$(oak_project_id)")
	out="$("$(oak_memlite)" --data-dir "$dir" remember "${args[@]}" --json "$content")"
	python3 - "$out" "$dir" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
o["tier"] = "project"
o["data_dir"] = sys.argv[2]
print(json.dumps(o, ensure_ascii=False))
PY
	;;

recall)
	query=""
	limit=5
	format="json"
	fallback=1
	while [ $# -gt 0 ]; do
		case "$1" in
		--limit) limit="${2:-5}"; shift 2 ;;
		--json) format="json"; shift ;;
		--format) format="${2:-md}"; shift 2 ;;
		--no-fallback) fallback=0; shift ;;
		-h | --help) sed -n '2,20p' "${BASH_SOURCE[0]}"; exit 0 ;;
		*) query="$1"; shift ;;
		esac
	done
	[ -n "$query" ] || { echo "oak-mem: recall 需要查询词" >&2; exit 2; }

	mem="$(oak_memlite)"
	proj="$(oak_project_mem)"
	user="$(oak_user_mem)"
	ensure_db "$proj"
	ensure_db "$user"
	oak_mem_sync_config

	proj_out="$("$mem" --data-dir "$proj" recall --limit "$limit" --json "$query" 2>/dev/null || echo '{"hits":[]}')"
	proj_hits="$(printf '%s' "$proj_out" | python3 -c 'import json,sys; print(len(json.load(sys.stdin).get("hits") or []))')"
	tier="project"
	res="$proj_out"
	if [ "$proj_hits" -eq 0 ] && [ "$fallback" -eq 1 ]; then
		res="$("$mem" --data-dir "$user" recall --limit "$limit" --json "$query" 2>/dev/null || echo '{"hits":[]}')"
		tier="user-fallback"
	fi

	python3 - "$res" "$query" "$tier" "$format" "$proj" "$user" <<'PY'
import json, sys
raw = sys.argv[1]
query, tier, fmt, proj, user = sys.argv[2:7]
try:
    data = json.loads(raw or "{}")
except json.JSONDecodeError:
    data = {"hits": [], "parse_error": True}
hits = data.get("hits") or []
# memlite 自带「工程库未命中 → 配置里的 data_dir」兜底；本脚本把配置钉在用户级库，
# 所以 fallback == "user" 时这些命中来自用户级，档位要如实标。
if data.get("fallback") == "user":
    tier = "user-fallback"
out = {
    "query": query,
    "tier": tier,
    "backend": data.get("backend", ""),
    "hits": [
        {"id": h.get("id"), "title": h.get("title", ""), "score": h.get("score"),
         "content": h.get("content", "")[:400], "tags": h.get("tags", [])}
        for h in hits
    ],
    "data_dir": proj if tier == "project" else user,
}
if fmt == "md":
    if not out["hits"]:
        print("_记忆检索无命中（工程库%s）。_" % ("与用户库均空" if tier == "user-fallback" else "为空"))
    else:
        print("档位：`%s`（backend=%s），命中 %d 条：" % (tier, out["backend"] or "bm25", len(out["hits"])))
        for h in out["hits"]:
            title = h["title"] or "（无标题）"
            print("- **%s** `%s` score=%.3f" % (title, h["id"], h["score"] or 0))
            body = h["content"].strip().replace("\n", " ")
            print("  > %s" % body)
        print()
        print("_以上是历史经验（参考），不是仓库事实；与当前代码冲突时以代码为准。_")
else:
    print(json.dumps(out, ensure_ascii=False, indent=2))
PY
	;;

show)
	id="${1:-}"
	[ -n "$id" ] || { echo "oak-mem: show 需要 id" >&2; exit 2; }
	for d in "$(oak_project_mem)" "$(oak_user_mem)"; do
		if [ -d "$d/.git" ]; then
			if out="$("$(oak_memlite)" --data-dir "$d" show --json "$id" 2>/dev/null)"; then
				printf '%s\n' "$out"
				exit 0
			fi
		fi
	done
	echo "oak-mem: 两档库中都找不到 $id" >&2
	exit 1
	;;

list)
	limit="${1:-10}"
	case "$limit" in --limit) limit="${2:-10}" ;; esac
	echo "== 工程库 $(oak_project_mem) =="
	"$(oak_memlite)" --data-dir "$(oak_project_mem)" list --limit "$limit" --json 2>/dev/null || true
	echo "== 用户库 $(oak_user_mem) =="
	"$(oak_memlite)" --data-dir "$(oak_user_mem)" list --limit "$limit" --json 2>/dev/null || true
	;;

status)
	echo "== 工程库 $(oak_project_mem) =="
	"$(oak_memlite)" --data-dir "$(oak_project_mem)" status --json 2>/dev/null || true
	echo "== 用户库 $(oak_user_mem) =="
	"$(oak_memlite)" --data-dir "$(oak_user_mem)" status --json 2>/dev/null || true
	;;

*)
	echo "oak-mem: 未知子命令 $cmd（remember|recall|show|list|status）" >&2
	exit 2
	;;
esac
