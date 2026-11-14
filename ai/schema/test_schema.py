#!/usr/bin/env python3
"""T0 tool-schema freeze tests for ai/schema/.

Run:  python3 ai/schema/test_schema.py     (exit code 0 = all green)

What is asserted
  A. schema syntax     - every tools/*.json parses, is draft-07, $id/title/field
                         naming conventions hold, metaschema-valid, no 'txn' field,
                         English-only descriptions.
  B. method-map drift  - schema file set <-> method-map table 1 (bidirectional);
                         methods extracted from OPP/1 section 8 <-> method-map
                         table 2 (bidirectional, mapped or deferred); extraction
                         count is asserted > 0 so a broken regex can never pass.
  C. samples           - every valid sample validates; every invalid sample fails
                         with the bound error keyword/path; required invalid
                         categories are all covered.
  D. hygiene           - no machine-local absolute paths, no git-excluded local
                         directory references, no API-key shaped tokens in the
                         committed deliverables.

The extractor deliberately reads only table rows of section 8 (first cell), so
non-method dotted mentions inside prose/code (e.g. an event name quoted in a
bullet) are not mistaken for methods.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

from jsonschema import Draft7Validator

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent
TOOLS_DIR = ROOT / "tools"
VALID_DIR = ROOT / "samples" / "valid"
INVALID_DIR = ROOT / "samples" / "invalid"
METHOD_MAP = ROOT / "method-map.md"
PROTOCOL = REPO / "docs" / "zh" / "plans" / "external-plugin-protocol.md"

DRAFT7 = "http://json-schema.org/draft-07/schema#"
TOOL_COUNT_BAND = (20, 24)  # T0 freezes "about 20" tools
METHOD_TOKEN_RE = re.compile(r"^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]+)+$")
FIELD_RE = re.compile(r"^[a-z][a-z0-9_]*$")
# CJK Unified Ideographs U+4E00..U+9FFF, spelled out in ASCII so this file
# itself stays free of non-English characters (repo rule: all things English).
CJK_RE = re.compile("[" + chr(0x4E00) + "-" + chr(0x9FFF) + "]")

# filename -> (tool, category, expected error keyword, expected error path)
INVALID_EXPECT = {
    "timeline.split_clip__bare-float-time.json":
        ("timeline.split_clip", "bare-float-time", "oneOf", ["time"]),
    "project.open__fps-bare-float.json":
        ("project.open", "bare-float-time", "oneOf", ["fps"]),
    "timeline.place_clip__timecode-string.json":
        ("timeline.place_clip", "wrong-type", "oneOf", ["in"]),
    "media.import_footage__wrong-type.json":
        ("media.import_footage", "wrong-type", "type", ["paths"]),
    "node.set_param__wrong-value-type.json":
        ("node.set_param", "wrong-type", "type", ["value"]),
    "timeline.split_clip__missing-clip.json":
        ("timeline.split_clip", "missing-required", "required", []),
    "render.get_frame__missing-target.json":
        ("render.get_frame", "missing-required", "oneOf", []),
    "node.set_param__temperature-out-of-range.json":
        ("node.set_param", "out-of-range", "maximum", ["value"]),
    "node.set_param__opacity-negative.json":
        ("node.set_param", "out-of-range", "minimum", ["value"]),
    "render.get_thumbnails__count-over-max.json":
        ("render.get_thumbnails", "out-of-range", "maximum", ["count"]),
    "timeline.trim_clip__illegal-side.json":
        ("timeline.trim_clip", "illegal-enum", "enum", ["side"]),
    "timeline.add_transition__illegal-type.json":
        ("timeline.add_transition", "illegal-enum", "enum", ["type"]),
    "render.get_frame__both-targets.json":
        ("render.get_frame", "invalid-target", "oneOf", []),
}
REQUIRED_INVALID_CATEGORIES = {
    "bare-float-time", "missing-required", "out-of-range", "illegal-enum", "wrong-type",
}

# Patterns are assembled from fragments so that this test file itself never
# contains the literals it scans for (the scan skips this file anyway).
FORBIDDEN_LITERALS = [
    "/" + "home" + "/",
    "/" + "Users" + "/",
    "ag" + "ent/",
    "AGENTS." + "override",
    "." + "pi/",
]
FORBIDDEN_RES = [re.compile(r"ak_[A-Za-z0-9]{8,}")]

records: list[tuple[str, str, bool, str]] = []


def check(section: str, ok: object, label: str, detail: str = "") -> None:
    records.append((section, label, bool(ok), detail))


def read_json(path: Path):
    try:
        return json.loads(path.read_text(encoding="utf-8")), None
    except Exception as exc:  # noqa: BLE001 - report, don't crash the suite
        return None, "%s: %s" % (type(exc).__name__, exc)


def tool_from_sample_name(stem: str) -> str:
    return stem.split("__", 1)[0]


def iter_property_names(schema: dict):
    """Yield every declared property name (root + nested object schemas)."""
    if not isinstance(schema, dict):
        return
    props = schema.get("properties")
    if isinstance(props, dict):
        for name in props:
            yield name
            yield from iter_property_names(props[name])
    for key in ("items", "additionalProperties"):
        if isinstance(schema.get(key), dict):
            yield from iter_property_names(schema[key])
    for key in ("allOf", "anyOf", "oneOf"):
        for sub in schema.get(key, []) if isinstance(schema.get(key), list) else []:
            yield from iter_property_names(sub)
    if isinstance(schema.get("definitions"), dict):
        for sub in schema["definitions"].values():
            yield from iter_property_names(sub)
    for key in ("if", "then", "else", "not"):
        if isinstance(schema.get(key), dict):
            yield from iter_property_names(schema[key])
    # 'if'/'then' blocks live inside allOf; also walk plain dict values that
    # are themselves schemas.
    for value in schema.values():
        if isinstance(value, dict) and ("properties" in value or "oneOf" in value):
            if value is not schema:
                yield from iter_property_names(value)
        elif isinstance(value, list):
            for item in value:
                if isinstance(item, dict) and ("properties" in item or "oneOf" in item):
                    yield from iter_property_names(item)


def section_a_schema_syntax() -> dict[str, dict]:
    schemas: dict[str, dict] = {}
    files = sorted(TOOLS_DIR.glob("*.json"))
    check("A", TOOLS_DIR.is_dir(), "tools/ directory exists")
    check("A", len(files) > 0, "tools/*.json present", "found %d" % len(files))
    check("A", TOOL_COUNT_BAND[0] <= len(files) <= TOOL_COUNT_BAND[1],
          "tool count within T0 band %d-%d (~20 frozen tools)"
          % TOOL_COUNT_BAND, "actual %d" % len(files))

    drafts = set()
    for path in files:
        tool = path.name[: -len(".json")]
        data, err = read_json(path)
        check("A", err is None, "%s: parses as JSON" % tool, err or "")
        if err is not None:
            continue
        schemas[tool] = data
        drafts.add(data.get("$schema"))
        check("A", data.get("$schema") == DRAFT7,
              "%s: draft-07 $schema" % tool, str(data.get("$schema")))
        check("A", data.get("$id") == "tools/" + path.name,
              "%s: $id convention (tools/<file>.json)" % tool, str(data.get("$id")))
        check("A", data.get("title") == tool, "%s: title == tool name" % tool,
              str(data.get("title")))
        check("A", data.get("type") == "object", "%s: root type object" % tool)
        check("A", isinstance(data.get("description"), str)
              and data["description"].strip() != "",
              "%s: non-empty description" % tool)
        props = data.get("properties")
        check("A", isinstance(props, dict), "%s: properties object" % tool)
        bad_fields = [n for n in iter_property_names(data)
                      if not FIELD_RE.match(str(n))]
        check("A", not bad_fields, "%s: field names snake_case" % tool,
              ",".join(sorted(set(map(str, bad_fields)))))
        check("A", "txn" not in (props or {}),
              "%s: no 'txn' field (LLM never sees the transaction token)" % tool)
        # upstream-facing text must be English
        text_bits = [json.dumps(data, ensure_ascii=False)]
        check("A", not any(CJK_RE.search(bit) for bit in text_bits),
              "%s: English-only schema text" % tool)
        try:
            Draft7Validator.check_schema(data)
            ok, detail = True, ""
        except Exception as exc:  # noqa: BLE001
            ok, detail = False, str(exc)
        check("A", ok, "%s: valid against draft-07 metaschema" % tool, detail)

    check("A", len(drafts) == 1, "all schemas share one draft",
          "drafts=%s" % sorted(map(str, drafts)))
    print("    tool schemas loaded: %d" % len(schemas))
    return schemas


def extract_protocol_methods() -> list[str]:
    text = PROTOCOL.read_text(encoding="utf-8")
    # Protocol headings are Chinese; anchor on the ASCII section numbers so
    # this test file stays English-only. Anchors verified unique in the doc.
    start = text.index("\n## 8. ") + 1
    end = text.index("\n## 9. ") + 1
    section = text[start:end]
    row_re = re.compile(
        r"^\|\s*`([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]+)+)`\s*\|", re.M)
    return sorted(set(row_re.findall(section)))


def parse_method_map() -> tuple[list[str], dict[str, str], dict[str, str]]:
    md = METHOD_MAP.read_text(encoding="utf-8")
    table1 = re.findall(r"^\|\s*\d+\s*\|\s*`([a-z0-9_.]+)`\s*\|", md, re.M)
    mapped: dict[str, str] = {}
    deferred: dict[str, str] = {}
    for line in md.splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 2:
            continue
        head = cells[0].strip("`")
        if not METHOD_TOKEN_RE.match(head):
            continue
        disposition = cells[1].strip()
        if disposition == "deferred":
            deferred[head] = cells[2] if len(cells) > 2 else ""
        elif disposition.startswith("`") and disposition.endswith("`"):
            mapped[head] = disposition.strip("`")
        else:
            check("B", False, "method-map: disposition of %s understood" % head,
                  disposition)
    return table1, mapped, deferred


def section_b_method_map(schemas: dict[str, dict]) -> None:
    check("B", METHOD_MAP.is_file(), "method-map.md exists")
    check("B", PROTOCOL.is_file(), "OPP protocol document exists", str(PROTOCOL))
    if not METHOD_MAP.is_file() or not PROTOCOL.is_file():
        return

    table1, mapped, deferred = parse_method_map()
    schema_names = set(schemas)
    table1_names = set(table1)

    check("B", len(table1) > 0, "method-map table 1 has tool rows",
          "rows=%d" % len(table1))
    check("B", table1_names == schema_names,
          "table 1 tools <-> schema files (bidirectional)",
          "only-in-map=%s only-in-dir=%s"
          % (sorted(table1_names - schema_names),
             sorted(schema_names - table1_names)))

    methods = extract_protocol_methods()
    check("B", len(methods) > 0,
          "section 8 method extraction yields > 0 methods", "count=%d" % len(methods))
    check("B", len(methods) >= 30,
          "section 8 method extraction sane (>= 30, guards regex rot)",
          "count=%d" % len(methods))

    map_rows = set(mapped) | set(deferred)
    check("B", map_rows == set(methods),
          "section 8 methods <-> method-map table 2 (bidirectional)",
          "protocol-only=%s map-only=%s"
          % (sorted(set(methods) - map_rows), sorted(map_rows - set(methods))))
    check("B", not (set(mapped) & set(deferred)),
          "no method both mapped and deferred",
          str(sorted(set(mapped) & set(deferred))))
    unmapped = sorted(set(methods) - map_rows)
    check("B", not unmapped,
          "every section 8 method has a tool or a deferred reason",
          ",".join(unmapped))
    bad_targets = sorted(t for t in mapped.values() if t not in schema_names)
    check("B", not bad_targets, "table 2 dispositions point at real tools",
          ",".join(bad_targets))
    missing_reason = sorted(m for m, r in deferred.items() if not r.strip())
    check("B", not missing_reason, "every deferred method carries a reason",
          ",".join(missing_reason))
    check("B", len(mapped) > 0 and len(deferred) > 0,
          "both mapped and deferred branches non-empty",
          "mapped=%d deferred=%d" % (len(mapped), len(deferred)))
    print("    OPP section 8 methods: %d (mapped %d, deferred %d)"
          % (len(methods), len(mapped), len(deferred)))


def section_c_samples(schemas: dict[str, dict]) -> None:
    valid_files = sorted(VALID_DIR.glob("*.json"))
    invalid_files = sorted(INVALID_DIR.glob("*.json"))
    check("C", len(valid_files) > 0, "valid samples present",
          "count=%d" % len(valid_files))
    check("C", len(invalid_files) > 0, "invalid samples present",
          "count=%d" % len(invalid_files))

    covered_tools = set()
    for path in valid_files:
        tool = tool_from_sample_name(path.stem)
        label = "valid/%s" % path.name
        check("C", tool in schemas, "%s: maps to a schema" % label, tool)
        if tool not in schemas:
            continue
        covered_tools.add(tool)
        args, err = read_json(path)
        check("C", err is None, "%s: parses as JSON" % label, err or "")
        if err is not None:
            continue
        validator = Draft7Validator(schemas[tool])
        errors = sorted(validator.iter_errors(args), key=lambda e: list(e.path))
        detail = "; ".join(e.message for e in errors[:3])
        check("C", not errors, "%s: validates against %s" % (label, tool), detail)

    missing = sorted(set(schemas) - covered_tools)
    check("C", not missing, "every tool has >= 1 valid sample", ",".join(missing))
    check("C", len(valid_files) >= len(schemas),
          "valid sample count >= tool count",
          "%d samples / %d tools" % (len(valid_files), len(schemas)))

    seen_names = {p.name for p in invalid_files}
    check("C", seen_names == set(INVALID_EXPECT),
          "invalid sample files == bound cases",
          "unbound=%s missing=%s" % (sorted(seen_names - set(INVALID_EXPECT)),
                                     sorted(set(INVALID_EXPECT) - seen_names)))
    categories = set()
    for name, (tool, category, keyword, path_parts) in sorted(INVALID_EXPECT.items()):
        label = "invalid/%s" % name
        check("C", tool in schemas, "%s: maps to a schema" % label, tool)
        if tool not in schemas:
            continue
        args, err = read_json(INVALID_DIR / name)
        check("C", err is None, "%s: parses as JSON" % label, err or "")
        if err is not None:
            continue
        categories.add(category)
        validator = Draft7Validator(schemas[tool])
        errors = list(validator.iter_errors(args))
        check("C", bool(errors), "%s: must be REJECTED" % label,
              "sample unexpectedly valid")
        matched = [e for e in errors
                   if e.validator == keyword and list(e.absolute_path) == path_parts]
        detail = "expected keyword=%s path=%s; got %s" % (
            keyword, path_parts,
            ["%s@%s" % (e.validator, list(e.absolute_path)) for e in errors][:4])
        check("C", bool(matched), "%s: fails with bound error" % label, detail)

    missing_cats = sorted(REQUIRED_INVALID_CATEGORIES - categories)
    check("C", not missing_cats,
          "invalid coverage: bare float / missing required / out of range / "
          "illegal enum / wrong type", ",".join(missing_cats))


def section_d_hygiene(schemas: dict[str, dict]) -> None:
    targets = [METHOD_MAP]
    targets += sorted(TOOLS_DIR.rglob("*.json"))
    targets += sorted(ROOT.glob("samples/**/*.json"))
    check("D", len(targets) > 0, "deliverable files located for hygiene scan",
          "count=%d" % len(targets))
    for path in targets:
        text = path.read_text(encoding="utf-8")
        rel = path.relative_to(ROOT)
        hits = [lit for lit in FORBIDDEN_LITERALS if lit in text]
        hits += [rx.pattern for rx in FORBIDDEN_RES if rx.search(text)]
        check("D", not hits, "%s: no local-only path / token literal" % rel,
              ",".join(hits))
        check("D", text.strip() != "", "%s: non-empty file" % rel)


def main() -> int:
    print("== T0 ai/schema test suite ==")
    print("repo: %s" % REPO)

    schemas = section_a_schema_syntax()
    if schemas:
        section_b_method_map(schemas)
        section_c_samples(schemas)
        section_d_hygiene(schemas)
    else:
        check("B", False, "skip: no schema loaded")
        check("C", False, "skip: no schema loaded")

    sections: dict[str, list[tuple[str, bool, str]]] = {}
    for section, label, ok, detail in records:
        sections.setdefault(section, []).append((label, ok, detail))

    total_pass = total_fail = 0
    for section in sorted(sections):
        rows = sections[section]
        passed = sum(1 for _, ok, _ in rows if ok)
        failed = len(rows) - passed
        total_pass += passed
        total_fail += failed
        print("[%s] %s  pass=%d fail=%d"
              % (section, "schema syntax" if section == "A"
                 else "method-map drift" if section == "B"
                 else "samples" if section == "C" else "hygiene",
                 passed, failed))
        for label, ok, detail in rows:
            if not ok:
                print("    FAIL  %s%s" % (label, (" -> " + detail) if detail else ""))

    print("-" * 60)
    print("TOTAL: pass=%d fail=%d" % (total_pass, total_fail))
    if total_fail:
        print("RESULT: FAILED")
        return 1
    print("RESULT: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
