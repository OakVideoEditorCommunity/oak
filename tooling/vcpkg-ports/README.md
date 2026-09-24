# vcpkg overlay ports

Passed to `vcpkg install` with `--overlay-ports tooling/vcpkg-ports`
(both workflows, see `.github/workflows/ci.yml` and
`.github/workflows/cd.yml`).

## x264

vcpkg's builtin x264 port downloads its source from `code.videolan.org`,
whose GitLab sits behind an anti-bot challenge: CI runners intermittently
failed the download (`curl` error 7 / HTTP 404), which made every vcpkg
install a coin flip. This overlay is a verbatim copy of the port at
baseline `771b0a2e7c473c3a6bee56553be4d5bb32bb653c` with
`vcpkg_from_gitlab` swapped for `vcpkg_from_github` against the
`mirror/x264` GitHub mirror. The mirror archive is byte-identical to
GitLab's (same SHA512), so only the fetch URL changes.

No other port in the dependency tree uses code.videolan.org (dav1d and
x265 already fetch from GitHub).

Refresh the copy (portfile, patches, `vcpkg.json`) whenever the
manifest's `builtin-baseline` moves so it keeps matching the resolved
version.
