# Oak / Quercus Code Style Guide

This guide applies to the Oak root repository, the standalone AI workspaces,
and the Quercus submodule. The goal is readable, maintainable, and verifiable
code, not mechanical rewrites that only make files look uniform.

## 1. Formatting

- Run `cargo fmt` before committing Rust code. Do not replace rustfmt with manual alignment.
- Run `cargo fmt --all` in the Oak root workspace and in the Quercus workspace.
- Format each standalone AI workspace separately:
  - `cargo fmt --all --manifest-path ai/oak-ai-tools/Cargo.toml`
  - `cargo fmt --all --manifest-path ai/oak-ai-mcp/Cargo.toml`
  - `cargo fmt --all --manifest-path ai/ci/Cargo.toml`
- Do not run multiple formatters concurrently when they can write the same files.
- Use the corresponding `cargo fmt --all -- --check` command to verify formatting before committing.

## 2. Files and Modules

- Keep files below 500 lines where practical.
- Split files above 1000 lines when they have a clear responsibility boundary. Do not cut tightly coupled logic merely to satisfy a line count.
- Large test modules may move out of implementation files without changing test names, module paths, or behavior.
- Split reusable domain logic by responsibility, such as parsing, state management, backend adapters, UI, and tests.
- A file may remain above 1000 lines when it is genuinely one semantic unit. Document the reason in the refactor log or code review.
- Do not mechanically split generated lockfiles, snapshots, or third-party protocol headers.
- Review vendored code as well, but keep changes consistent with local patch responsibilities and avoid unnecessary synchronization costs.

## 3. Imports and Paths

- Prefer `use` imports over fully qualified paths where practical, even for symbols used only once.
- Place imports in the module scope that actually uses them. A local `use` inside a function does not cover the whole module.
- Choose module or symbol imports that preserve clear domain meaning. Do not sacrifice readability merely to shorten a path.
- Keep fully qualified paths for `thiserror::Error`, naming conflicts, macro paths, documentation links, conditional compilation, and other special resolution cases.
- Do not import the same symbol twice in one module or rely on ambiguous resolution between same-named symbols from different sources.
- Bulk path rewrites must skip strings, comments, and macro bodies and must be validated by the compiler. Never rely on global text replacement alone.
- Do not use glob imports to hide naming conflicts. Do not mechanically expand existing patterns such as `use super::*`.

## 4. Dependency Selection

- Prefer mature, maintained, license-compatible libraries over new implementations of protocol parsing, serialization, error handling, concurrency, time, cryptography, or platform integration.
- Search Oak, Quercus, and every standalone workspace before adding a dependency. Avoid introducing a second implementation of the same functionality.
- Reuse existing dependencies and features where possible, including the established error derive, JSON, async runtime, MCP, file-locking, and process-management solutions.
- Do not add multiple versions or implementations of similar dependencies without documenting the compatibility boundary and the reason for coexistence.
- Evaluate maintenance status, security history, licensing, platform support, MSRV, build cost, binary size, and reproducibility in offline CI.
- Do not introduce a large framework for a small domain-specific helper. Preferring mature libraries does not mean maximizing dependency count.
- Pin versions and update the relevant lockfiles. Standalone AI workspaces must not assume that the root workspace resolves their dependencies.

## 5. Implementation Style

- Fix root causes and reuse existing abstractions instead of adding parallel temporary paths.
- Keep module boundaries consistent with the codebase. Do not mix behavior changes into style-only refactors.
- Use names that communicate responsibility and avoid unclear abbreviations and redundant wrappers.
- Preserve structured error context. Do not hide missing capabilities behind silent fallbacks, empty implementations, or false success.
- Use comments and documentation to explain constraints and rationale rather than translating code line by line.
- Do not change public APIs, test semantics, configuration defaults, or resource contents in style-only changes.

## 6. Tests and Commits

- Run the tests most relevant to the changed files first, then workspace tests and the local equivalent of the affected CI jobs.
- After moving tests, compile the test targets, not only the library. Private-item visibility is a common failure point.
- After path cleanup, fix compiler errors and new warnings, especially mistakes involving local scopes and conditional compilation.
- Keep AI headless tests offline, key-free, and replayable. Separate dependency downloads from offline test execution.
- Keep style changes in dedicated commits and do not mix them with feature work.
- Include CI fixes caused by the refactor in the same style commit. Record unrelated failures without changing unrelated functionality.
- Do not include pre-existing user files that were not explicitly requested in the commit.

## 7. Review Checklist

1. The change covers every required repository, not just files open in the editor.
2. All applicable workspaces have been formatted with `cargo fmt`.
3. Oversized files have either been split appropriately or have a documented reason to remain intact.
4. Fully qualified paths have been converted where safe, with conflicts and special resolution cases documented.
5. No duplicate functionality or unnecessary dependencies have been introduced, and existing mature implementations are reused.
6. Relevant tests, workspace test builds, and AI headless CI have passed.
7. The style commit contains no feature changes, temporary scripts, build artifacts, or unauthorized pre-existing files.
