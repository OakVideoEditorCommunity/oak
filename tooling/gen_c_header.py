#!/usr/bin/env python3
"""Generate the C header for the engine ABI surface used by the Rust SDK.

The engine exports a large `extern "C"` surface. The Rust SDK is the
authoritative consumer: its `unsafe extern "C"` blocks and `#[repr(C)]`
mirror types define exactly the stable ABI a new frontend needs. This tool
turns those declarations into a self-contained C header so C/C++ hosts can
call the same functions instead of inventing their own declarations.

Usage:
    python3 tooling/gen_c_header.py            # print to stdout
    python3 tooling/gen_c_header.py --write    # write include/oak_engine.h

The header is checked in; CI runs this with `--check` to catch drift.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SDK = ROOT / "crates" / "oak-engine-rust-sdk" / "src"
HEADER = ROOT / "crates" / "oak-engine" / "include" / "oak_engine.h"

PRIMITIVES = {
    "bool": "bool",
    "u8": "uint8_t",
    "u16": "uint16_t",
    "u32": "uint32_t",
    "u64": "uint64_t",
    "usize": "size_t",
    "i8": "int8_t",
    "i16": "int16_t",
    "i32": "int32_t",
    "i64": "int64_t",
    "isize": "ptrdiff_t",
    "f32": "float",
    "f64": "double",
    "c_void": "void",
    "c_char": "char",
}

# Newtypes that are passed as their scalar representation in the ABI.
NEWTYPES = {
    "NodeId": "uint64_t",
    "Cursor": "uint64_t",
}

# Rust type aliases/mirrors whose C spelling differs only by path stripping.
RENAME = {
    "OakTimeRange": "OakTimeRange",
}

# C++ keywords that appear as ABI field/parameter names in the Rust mirrors.
CPP_KEYWORDS = {
    "this", "new", "delete", "class", "template", "typename", "operator",
    "private", "public", "protected", "virtual", "namespace", "friend",
    "explicit", "mutable", "using", "bool", "true", "false", "nullptr",
    "const", "static", "register", "auto", "union", "enum", "struct",
    "default", "case", "switch", "throw", "try", "catch", "export",
    "extern", "inline", "signed", "unsigned", "volatile", "wchar_t",
}


def c_ident(name: str) -> str:
    return name + "_" if name in CPP_KEYWORDS else name


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    text = re.sub(r"//[^\n]*", "", text)
    return text


def top_level_split(text: str, separator: str = ",") -> list[str]:
    parts: list[str] = []
    depth = 0
    current = []
    for char in text:
        if char in "([{<":
            depth += 1
        elif char in ")]}>":
            depth -= 1
        if char == separator and depth == 0:
            parts.append("".join(current))
            current = []
        else:
            current.append(char)
    if "".join(current).strip():
        parts.append("".join(current))
    return parts


def find_matching(text: str, start: int, open_char: str, close_char: str) -> int:
    depth = 0
    for index in range(start, len(text)):
        if text[index] == open_char:
            depth += 1
        elif text[index] == close_char:
            depth -= 1
            if depth == 0:
                return index
    raise ValueError("unbalanced source")


def shorten(path: str) -> str:
    path = path.strip()
    path = re.sub(r"^(crate|std|core|oak_engine_rust_sdk)::", "", path)
    path = path.split("::")[-1]
    return path


class TypeMapper:
    def __init__(self) -> None:
        self.structs: dict[str, str] = {}  # name -> emitted definition
        self.deps: dict[str, set[str]] = {}
        self.enums: dict[str, str] = {}  # name -> storage typedef

    def map(self, rust: str, parameter: str | None = None) -> str:
        text = rust.strip()
        if text.startswith("Option<") and text.endswith(">"):
            inner = text[len("Option<") : -1]
            if "fn" in inner:
                return self.callback(inner, parameter)
            return self.map(inner, parameter)
        if text.startswith("*const "):
            inner = self.map(text[7:])
            return f"{inner}*" if inner.startswith("const ") else f"const {inner}*"
        if text.startswith("*mut "):
            return f"{self.map(text[5:])}*"
        if text.startswith("&"):
            inner = text.lstrip("&").strip()
            if inner.startswith("mut "):
                return f"{self.map(inner[4:])}*"
            return f"const {self.map(inner)}*"
        if text.startswith("[") and text.endswith("]"):
            inner, count = top_level_split(text[1:-1], ";")
            return f"{self.map(inner)}"
        if text == "Mat3" or shorten(text) == "Mat3":
            return "OakMat3"
        if re.match(r"^\[\[.+;\s*\d+\];\s*\d+\]$", text, re.S):
            return "OakMat3"
        base = shorten(text)
        if base in PRIMITIVES:
            return PRIMITIVES[base]
        if base in NEWTYPES:
            return NEWTYPES[base]
        return RENAME.get(base, base)

    def declare(self, rust: str, name: str) -> str:
        """One C declaration: callback types carry the name inside the parens."""
        text = rust.strip()
        if text.startswith("Option<") and text.endswith(">"):
            inner = text[len("Option<") : -1]
            if "fn" in inner:
                return self.callback(inner, name)
        return f"{self.map(text)} {name}"

    def callback(self, inner: str, parameter: str | None) -> str:
        match = re.search(r"fn\s*\((.*?)\)", inner)
        args = ""
        if match and match.group(1).strip():
            mapped = [self.map(arg) for arg in top_level_split(match.group(1))]
            args = ", ".join(mapped)
        name = f" {parameter}" if parameter else ""
        return f"void (*{name})({args})" if parameter else f"void (*)({args})"


def parse_structs(source: str, mapper: TypeMapper) -> list[str]:
    output: list[str] = []
    for match in re.finditer(r"#\[repr\(C\)\]", source):
        tail = source[match.end() :]
        struct = re.match(
            r"\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\(crate\))?\s+)?struct (\w+)\s*\{",
            tail,
        )
        if not struct:
            continue
        name = struct.group(1)
        brace = source.find("{", match.end())
        if brace < 0:
            continue
        end = find_matching(source, brace, "{", "}")
        body = source[brace + 1 : end]
        if re.search(r"\bimpl\b|\$", body):
            continue
        fields: list[str] = []
        raw_types: list[str] = []
        for field in top_level_split(body):
            field = field.strip()
            if not field or field.startswith("#"):
                continue
            field = re.sub(r"#\[[^\]]*\]", "", field).strip()
            field = re.sub(r"^pub\s+", "", field)
            if ":" not in field:
                continue
            fname, ftype = field.split(":", 1)
            fname = c_ident(fname.strip())
            ftype = ftype.strip()
            raw_types.append(ftype)
            array = re.match(r"^\[(.+);\s*(\d+)\]$", ftype, re.S)
            if array:
                ctype = mapper.map(array.group(1))
                fields.append(f"    {ctype} {fname}[{array.group(2)}];")
            else:
                fields.append(f"    {mapper.declare(ftype, fname)};")
        definition = f"struct {name} {{\n" + "\n".join(fields) + "\n};"
        mapper.structs[name] = definition
        mapper.deps[name] = set()
        output.append((name, definition, raw_types))
    return output


def parse_enums(source: str, mapper: TypeMapper) -> list[str]:
    output: list[str] = []
    for match in re.finditer(r"#\[repr\((C|u32|i32|u8|i8)\)\]", source):
        repr_kind = match.group(1)
        tail = source[match.end() :]
        enum = re.match(
            r"\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\(crate\))?\s+)?enum (\w+)",
            tail,
        )
        if not enum:
            continue
        name = enum.group(1)
        brace = source.find("{", match.end())
        if brace < 0:
            continue
        end = find_matching(source, brace, "{", "}")
        if re.search(r"\bimpl\b|\$", source[brace + 1 : end]):
            continue
        variants = []
        next_value = 0
        for variant in top_level_split(source[brace + 1 : end]):
            variant = re.sub(r"#\[[^\]]*\]", "", variant).strip()
            if not variant:
                continue
            m = re.match(r"(\w+)(?:\s*=\s*(-?\w+))?", variant)
            if not m:
                continue
            vname = m.group(1)
            if m.group(2) is not None:
                next_value = int(m.group(2), 0)
            variants.append((vname, next_value))
            next_value += 1
        prefix = name[3:] if name.startswith("Oak") else name
        if repr_kind == "C" or repr_kind in ("i32",):
            body = ",\n".join(
                f"    OAK_{prefix.upper()}_{vname.upper()} = {value}"
                for vname, value in variants
            )
            output.append(f"typedef enum {name} {{\n{body}\n}} {name};")
            mapper.enums[name] = name
        else:
            storage = "uint32_t" if repr_kind == "u32" else "uint8_t"
            defines = "\n".join(
                f"#define OAK_{prefix.upper()}_{vname.upper()} {value}U"
                for vname, value in variants
            )
            output.append(f"typedef {storage} {name};\n{defines}")
            mapper.enums[name] = storage
    return output


def parse_type_aliases(source: str, mapper: TypeMapper) -> list[str]:
    output: list[str] = []
    for match in re.finditer(
        r"type\s+(\w+)\s*=\s*(?:Option<)?\s*unsafe extern \"C\" fn\s*\((.*?)\)",
        source,
        re.S,
    ):
        name, args = match.group(1), match.group(2)
        mapped = ", ".join(
            (
                lambda aname, atype: mapper.declare(atype, c_ident(aname))
            )(*arg.split(":", 1)) if ":" in arg else mapper.map(arg.strip())
            for arg in top_level_split(args)
        )
        output.append(f"typedef void (*{name})({mapped});")
    return output


def extern_blocks(source: str) -> list[str]:
    blocks: list[str] = []
    for match in re.finditer(r'unsafe extern "C"\s*\{', source):
        brace = match.end() - 1
        end = find_matching(source, brace, "{", "}")
        blocks.append(source[brace + 1 : end])
    return blocks


def parse_functions(source: str, mapper: TypeMapper) -> list[str]:
    declarations: dict[str, str] = {}
    for body in extern_blocks(source):
        index = 0
        while True:
            match = re.search(r"\bfn\s+(\w+)\s*\(", body[index:])
            if not match:
                break
            name = match.group(1)
            if not name.startswith("oak_"):
                index = index + match.end()
                continue
            open_paren = index + match.end() - 1
            close_paren = find_matching(body, open_paren, "(", ")")
            args_text = body[open_paren + 1 : close_paren]
            rest = body[close_paren + 1 :]
            arrow = re.match(r"\s*->\s*", rest)
            if not arrow:
                if rest.lstrip().startswith(";"):
                    args = []
                    for arg in top_level_split(args_text):
                        arg = arg.strip()
                        if ":" not in arg:
                            continue
                        aname, atype = arg.split(":", 1)
                        args.append(mapper.declare(atype, c_ident(aname.strip())))
                    declarations.setdefault(
                        name, f"void {name}({', '.join(args) or 'void'});"
                    )
                index = close_paren + 1
                continue
            start = close_paren + 1 + arrow.end()
            depth = 0
            end = start
            while end < len(body):
                char = body[end]
                if char in "([{<":
                    depth += 1
                elif char in ")]}>":
                    depth -= 1
                elif char == ";" and depth == 0:
                    break
                end += 1
            ret = body[start:end].strip()
            ret_end = end
            args = []
            for arg in top_level_split(args_text):
                arg = arg.strip()
                if not arg:
                    continue
                if ":" not in arg:
                    continue
                aname, atype = arg.split(":", 1)
                args.append(mapper.declare(atype, c_ident(aname.strip())))
            prototype = f"{mapper.map(ret)} {name}({', '.join(args) or 'void'});"
            declarations.setdefault(name, prototype)
            index = ret_end + 1
    return [declarations[name] for name in sorted(declarations)]


def generate() -> str:
    sources = "\n".join(
        strip_comments(path.read_text(encoding="utf-8"))
        for path in sorted(SDK.rglob("*.rs"))
    )
    mapper = TypeMapper()
    structs = parse_structs(sources, mapper)
    enums = parse_enums(sources, mapper)
    for name, _definition, raw_types in structs:
        for raw in raw_types:
            for candidate in mapper.structs:
                if candidate != name and re.search(rf"\b{candidate}\b", raw):
                    mapper.deps[name].add(candidate)
    ordered: list[str] = []
    visiting: set[str] = set()

    def visit(name: str) -> None:
        if name in ordered or name in visiting:
            return
        visiting.add(name)
        for dep in sorted(mapper.deps.get(name, ())):
            visit(dep)
        visiting.discard(name)
        ordered.append(name)

    for name in sorted(mapper.structs):
        visit(name)
    structs = [mapper.structs[name] for name in ordered]
    functions = parse_functions(sources, mapper)
    callbacks = parse_type_aliases(sources, mapper)
    parts = [
        "/* SPDX-License-Identifier: GPL-3.0-or-later */",
        "/* Oak engine C ABI — generated by tooling/gen_c_header.py. Do not edit. */",
        "#ifndef OAK_ENGINE_H",
        "#define OAK_ENGINE_H",
        "",
        "#include <stdbool.h>",
        "#include <stddef.h>",
        "#include <stdint.h>",
        "",
        "#ifdef __cplusplus",
        'extern "C" {',
        "#endif",
        "",
        f"/* {len(structs)} POD structs, {len(enums)} enums, {len(functions)} functions. */",
        "",
        "/* 3x3 color matrix returned by the colormath helpers. */",
        "typedef struct OakMat3 { float v[3][3]; } OakMat3;",
        "",
    ]
    parts.extend(enums)
    parts.append("")
    parts.extend(f"typedef struct {name} {name};" for name in ordered)
    parts.append("")
    parts.extend(structs)
    parts.append("")
    parts.extend(
        [
            "/* Engine-side spellings for the common POD mirrors. */",
            "typedef Rational OakRational;",
            "typedef TimeRange OakTimeRange;",
            "typedef NodeValue OakNodeValue;",
            "",
            "/* Layout sanity checks for the most commonly embedded mirrors. */",
            "#if defined(__cplusplus)",
            "static_assert(sizeof(Rational) == 16, \"Rational ABI layout\");",
            "static_assert(sizeof(TimeRange) == 32, \"TimeRange ABI layout\");",
            "static_assert(sizeof(NodeValue) == 56, \"NodeValue ABI layout\");",
            "static_assert(sizeof(StorageProjectHandle) == 32, \"StorageProjectHandle ABI layout\");",
            "#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L",
            "_Static_assert(sizeof(Rational) == 16, \"Rational ABI layout\");",
            "_Static_assert(sizeof(TimeRange) == 32, \"TimeRange ABI layout\");",
            "_Static_assert(sizeof(NodeValue) == 56, \"NodeValue ABI layout\");",
            "_Static_assert(sizeof(StorageProjectHandle) == 32, \"StorageProjectHandle ABI layout\");",
            "#endif",
            "",
        ]
    )
    parts.extend(callbacks)
    parts.append("")
    parts.extend(functions)
    parts.extend(
        [
            "",
            "#ifdef __cplusplus",
            "}",
            "#endif",
            "",
            "#endif /* OAK_ENGINE_H */",
            "",
        ]
    )
    return "\n".join(parts)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="write the checked-in header")
    parser.add_argument("--check", action="store_true", help="fail when the header is stale")
    args = parser.parse_args()
    header = generate()
    if args.write:
        HEADER.parent.mkdir(parents=True, exist_ok=True)
        HEADER.write_text(header, encoding="utf-8")
        print(f"wrote {HEADER.relative_to(ROOT)}")
        return 0
    if args.check:
        current = HEADER.read_text(encoding="utf-8") if HEADER.exists() else ""
        if current != header:
            print("oak_engine.h is stale; run python3 tooling/gen_c_header.py --write", file=sys.stderr)
            return 1
        print("oak_engine.h is up to date")
        return 0
    sys.stdout.write(header)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
