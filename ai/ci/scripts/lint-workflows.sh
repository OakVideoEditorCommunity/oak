#!/usr/bin/env bash
# GitHub workflow 静态检查：
#   1) 有 actionlint 就用它（最严）；
#   2) 没有则退化为 PyYAML 解析 + 必要结构断言（on/jobs/permissions/uses）。
# 本机没有 actionlint 时输出 "fallback"，回报里如实标注。
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
cd "$repo"

files=(.github/workflows/*.yml .github/workflows/*.yaml)
files=($(printf '%s\n' "${files[@]}" | while read -r f; do [ -f "$f" ] && echo "$f"; done))
[ "${#files[@]}" -gt 0 ] || { echo "no workflow files"; exit 1; }

if command -v actionlint >/dev/null 2>&1; then
	echo "[lint] actionlint:"
	actionlint "${files[@]}"
	exit $?
fi

echo "[lint] actionlint 不在本机，退化为 PyYAML 解析 + 结构断言（首次推 GitHub 后以 Actions 运行为准）"
python3 - "${files[@]}" <<'PY'
import sys, yaml, pathlib

fail = 0
for name in sys.argv[1:]:
    path = pathlib.Path(name)
    try:
        doc = yaml.safe_load(path.read_text())
    except yaml.YAMLError as err:
        print(f"FAIL {name}: YAML 解析错误: {err}")
        fail = 1
        continue
    if not isinstance(doc, dict):
        print(f"FAIL {name}: 顶层不是映射")
        fail = 1
        continue
    problems = []
    # PyYAML 会把 on 解析成 True（YAML 1.1），两种键都要认
    if "on" not in doc and True not in doc:
        problems.append("缺 on:")
    jobs = doc.get("jobs")
    if not isinstance(jobs, dict) or not jobs:
        problems.append("缺 jobs:")
    else:
        for jname, job in jobs.items():
            if not isinstance(job, dict):
                problems.append(f"job {jname} 不是映射")
                continue
            if "runs-on" not in job:
                problems.append(f"job {jname} 缺 runs-on")
            steps = job.get("steps")
            if not isinstance(steps, list) or not steps:
                problems.append(f"job {jname} 缺 steps")
            else:
                for i, st in enumerate(steps):
                    if not isinstance(st, dict) or ("run" not in st and "uses" not in st):
                        problems.append(f"job {jname} step {i} 既无 run 也无 uses")
                    uses = st.get("uses", "")
                    if isinstance(uses, str) and uses and "@" not in uses:
                        problems.append(f"job {jname} step {i} uses 缺版本钉: {uses}")
    # 禁止把密钥写进 workflow
    text = path.read_text()
    for bad in ("api_key:", "apiKey:", "secret ", "BEGIN PRIVATE KEY"):
        if bad in text:
            problems.append(f"疑似密钥片段: {bad!r}")
    if problems:
        print(f"FAIL {name}: " + "; ".join(problems))
        fail = 1
    else:
        print(f"OK   {name}: jobs={list((jobs or {}).keys())}")
sys.exit(fail)
PY
