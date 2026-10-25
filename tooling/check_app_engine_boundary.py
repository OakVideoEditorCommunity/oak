#!/usr/bin/env python3
"""Ratchet oak-app's direct dependencies on Oak implementation crates.

The checked-in baseline is a ceiling, not an allowlist to grow. New files
with references, new direct dependencies, and increases at either the file
or per-crate level fail. Run with --update-baseline after reducing usage;
the same checks are applied before the baseline is rewritten.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "crates" / "oak-app"
BASELINE = ROOT / "tooling" / "app-engine-boundary-baseline.json"
CRATES = (
    "oak-audio",
    "oak-codec",
    "oak-core",
    "oak-engine",
    "oak-node",
    "oak-ofx-plugin",
    "oak-otio",
    "oak-render",
    "oak-storage",
    "oak-task",
    "oak-timeline",
    "oak-undo",
)
REFERENCE = re.compile(
    r"\boak_(?:audio|codec|core|engine|node|ofx_plugin|otio|render|storage|task|timeline|undo)::"
)
CRATE_REFERENCE = re.compile(r"\boak_(?:audio|codec|core|engine|node|ofx_plugin|otio|render|storage|task|timeline|undo)(?=::)")
DEPENDENCY_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")


def source_files() -> list[Path]:
    files = list((APP / "src").rglob("*.rs"))
    if (APP / "tests").is_dir():
        files.extend((APP / "tests").rglob("*.rs"))
    return sorted(files)


def direct_dependencies(manifest: dict) -> dict[str, list[str]]:
    found: dict[str, set[str]] = {section: set() for section in DEPENDENCY_SECTIONS}

    def inspect(section: str, values: object) -> None:
        if not isinstance(values, dict):
            return
        for alias, declaration in values.items():
            package = alias
            if isinstance(declaration, dict):
                package = declaration.get("package", alias)
            if package in CRATES:
                found[section].add(package)

    for section in DEPENDENCY_SECTIONS:
        inspect(section, manifest.get(section))
    for target in manifest.get("target", {}).values():
        if isinstance(target, dict):
            for section in DEPENDENCY_SECTIONS:
                inspect(section, target.get(section))
    return {section: sorted(names) for section, names in found.items()}


def collect() -> dict:
    references = {}
    for path in source_files():
        text = path.read_text(encoding="utf-8")
        crates: dict[str, int] = {}
        for match in CRATE_REFERENCE.finditer(text):
            name = match.group(0).replace("_", "-")
            # Rust module names map back to Cargo package names.
            crates[name] = crates.get(name, 0) + 1
        total = len(REFERENCE.findall(text))
        if total:
            relative = path.relative_to(ROOT).as_posix()
            references[relative] = {"total": total, "crates": dict(sorted(crates.items()))}
    with (APP / "Cargo.toml").open("rb") as stream:
        manifest = tomllib.load(stream)
    return {
        "schema": 1,
        "dependencies": direct_dependencies(manifest),
        "references": dict(sorted(references.items())),
    }


def total_references(snapshot: dict) -> int:
    return sum(item["total"] for item in snapshot["references"].values())


def violations(baseline: dict, current: dict) -> list[str]:
    errors = []
    for section in DEPENDENCY_SECTIONS:
        before = set(baseline["dependencies"].get(section, []))
        after = set(current["dependencies"].get(section, []))
        for name in sorted(after - before):
            errors.append(f"new direct dependency in [{section}]: {name}")

    before_files = baseline["references"]
    after_files = current["references"]
    for path, after in after_files.items():
        before = before_files.get(path)
        if before is None:
            errors.append(f"new file with internal references: {path} ({after['total']})")
            continue
        if after["total"] > before["total"]:
            errors.append(
                f"reference count increased: {path}: {before['total']} -> {after['total']}"
            )
        for crate, count in after["crates"].items():
            old_count = before["crates"].get(crate, 0)
            if count > old_count:
                errors.append(
                    f"{crate} references increased: {path}: {old_count} -> {count}"
                )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update-baseline", action="store_true")
    args = parser.parse_args()
    current = collect()
    if not BASELINE.exists():
        if not args.update_baseline:
            print(f"missing baseline: {BASELINE.relative_to(ROOT)}", file=sys.stderr)
            return 1
        baseline = current
    else:
        baseline = json.loads(BASELINE.read_text(encoding="utf-8"))
        if baseline.get("schema") != 1:
            print("unsupported boundary baseline schema", file=sys.stderr)
            return 1

    errors = violations(baseline, current)
    if errors:
        print("oak-app engine boundary regressed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        print(
            "Move the capability behind oak-engine-rust-sdk; do not grow the ratchet baseline.",
            file=sys.stderr,
        )
        return 1

    before = total_references(baseline)
    after = total_references(current)
    if args.update_baseline:
        BASELINE.write_text(
            json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        print(f"updated boundary baseline: {before} -> {after} internal references")
    else:
        dependencies = sum(len(names) for names in current["dependencies"].values())
        print(
            f"oak-app boundary OK: {after} internal references in "
            f"{len(current['references'])} files, {dependencies} direct internal dependencies "
            f"(baseline {before})"
        )
        if after < before:
            print("baseline may be reduced with --update-baseline")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
