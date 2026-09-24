# Oak's build-type overlay triplet for x64-windows (see the note in
# ../README.md): the settings mirror vcpkg's builtin x64-windows.cmake and
# only the build type is pinned. Pass this directory with
# `--overlay-triplets tooling/vcpkg-triplets/release`.
set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE dynamic)
set(VCPKG_PROVIDED_FORTRAN ON)

# Build only the release configuration (see x64-linux.cmake).
set(VCPKG_BUILD_TYPE release)
