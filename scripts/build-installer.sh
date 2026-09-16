#!/usr/bin/env bash
# Builds the Linux packages the way a release is built, and refuses to leave a bad one on disk.
#
# `scripts/build-installer.ps1` in shell spelling. Five things whose order matters, which is
# why this is one script and not five steps:
#
#   1. the tray check, because the *name of a dependency* in the finished package is decided
#      by what pkg-config can see on this machine — see below, it is the surprising one
#   2. the remapped environment, so no cargo call below can compile a machine path in
#   3. the sidecars (scripts/build-host.sh) — the engine host and the command line — because
#      `tauri-build` checks for them before dile-app compiles
#   4. `cargo tauri build`, which bundles the .deb and the .rpm
#   5. scripts/check-binary-paths.sh over all three programs
#
# Step 5 is not a courtesy. Panic locations are compiled in as string literals, `strip` does
# not remove them, and a release build can therefore carry the path every crate was compiled
# from — on Linux that is `/home/<account>`, which is usually a person's name, published. If
# the check finds one, **the bundle is deleted** rather than left on disk looking finished, so
# a package that exists after this script ran is one that passed.
#
# Usage:
#   scripts/build-installer.sh                  # what a release is built with
#   scripts/build-installer.sh --skip-sidecars  # a second run that only changed the app
#   scripts/build-installer.sh --legacy-tray    # a machine with no ayatana development files

set -uo pipefail

skip_sidecars=0
legacy_tray=0

while [ $# -gt 0 ]; do
    case "$1" in
        --skip-sidecars | -SkipSidecars)
            skip_sidecars=1
            shift
            ;;
        --legacy-tray)
            legacy_tray=1
            shift
            ;;
        --help | -h)
            sed -n '2,25p' "$0"
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_directory}/.." && pwd)"
target_root="${CARGO_TARGET_DIR:-${repo_root}/target}"

dim() { printf '\033[90m%s\033[0m\n' "$1"; }
red() { printf '\033[31m%s\033[0m\n' "$1"; }
yellow() { printf '\033[33m%s\033[0m\n' "$1"; }

# ---------------------------------------------------------------------------------------
# 1. The tray, and why it is the first thing rather than a detail.
#
# `tauri-cli` writes the appindicator dependency into the .deb and the .rpm from what
# **pkg-config on the building machine** can see: with `ayatana-appindicator3-0.1` present the
# packages depend on the maintained library, and without it they silently name the 2018
# `libappindicator3-1` instead — a package that is gone from Debian 13 and deprecated
# everywhere else. Nothing fails, nothing is printed, and the mistake is only visible in the
# finished package's metadata.
#
# So it is checked here, before anything is compiled, and it stops rather than warns: a
# published package that asks for the wrong library is worse than one that does not exist.
# `--legacy-tray` is the escape for trying the packaging on a machine without the development
# files, and the summary at the end says what it produced.
# ---------------------------------------------------------------------------------------

if pkg-config --exists ayatana-appindicator3-0.1 2>/dev/null; then
    export TAURI_LINUX_AYATANA_APPINDICATOR=true
    tray="ayatana"
elif [ "${legacy_tray}" -eq 1 ]; then
    tray="legacy"
else
    red "The ayatana appindicator development files are not installed."
    echo
    echo "Without them the packages will name the 2018 libappindicator instead of the"
    echo "maintained one, quietly, in a field nobody reads until an install fails:"
    echo
    echo "  sudo dnf install libayatana-appindicator-gtk3-devel     # Fedora"
    echo "  sudo apt install libayatana-appindicator3-dev           # Debian and Ubuntu"
    echo
    echo "Or pass --legacy-tray to build one anyway, which is not a release."
    exit 1
fi

# ---------------------------------------------------------------------------------------
# 2. The remapped environment.
#
# Three prefixes, which is every one that shows up in practice on this toolchain. The standard
# library arrives already remapped by the Rust project as `/rustc/<hash>/...`, and nothing here
# builds it from source; `.rustup` is a pattern in the checker rather than a prefix here, so a
# toolchain that ever did would go red instead of quietly shipping.
#
# The registry maps to `cargo` rather than to `crates`, because this workspace keeps its own
# code in `crates/` and a dependency panicking from `crates/serde_json-1.0.x/src/...` would
# read like one of ours.
#
# CARGO_ENCODED_RUSTFLAGS and not RUSTFLAGS: the unencoded variable is split on whitespace, and
# a home directory with a space in it would break every flag above in a way that looks like a
# compiler bug. Cargo reads the encoded one in preference and ignores the other entirely when
# it is set, so whatever the caller asked for is carried across rather than replaced.
# ---------------------------------------------------------------------------------------

cargo_home="${CARGO_HOME:-${HOME}/.cargo}"
separator=$'\037'

inherited=""
if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
    inherited="${CARGO_ENCODED_RUSTFLAGS}${separator}"
elif [ -n "${RUSTFLAGS:-}" ]; then
    inherited="$(printf '%s' "${RUSTFLAGS}" | tr ' ' "${separator}")${separator}"
fi

export CARGO_ENCODED_RUSTFLAGS="${inherited}--remap-path-prefix=${cargo_home}/registry/src=cargo${separator}--remap-path-prefix=${cargo_home}/git/checkouts=git${separator}--remap-path-prefix=${repo_root}=dile"

# --------------------------------------------------------------------------------------
# And the same thing for the C and C++ half, which --remap-path-prefix cannot reach.
#
# `transcribe-cpp-sys` compiles ggml from source through CMake, and every GGML_ASSERT and
# GGML_ABORT in it carries `__FILE__`. On Windows that needed MSVC's undocumented
# `/d1trimfile:`; here it is `-ffile-prefix-map=`, which gcc and clang have both had for years
# and which does the same job in the open.
#
# The cmake crate appends CFLAGS and CXXFLAGS to what it derives itself, so this adds rather
# than replaces, and the sys crate's own optimisation flags are untouched.
#
# **Cargo does not rebuild on a CFLAGS change**, because the sys crate declares no
# `rerun-if-env-changed` for it. A tree that already built the native library without this
# keeps the library it has, and the check at the end is what notices. Its failure message names
# the one command that fixes it.
# --------------------------------------------------------------------------------------

file_map="-ffile-prefix-map=${cargo_home}/registry/src=cargo -ffile-prefix-map=${repo_root}=dile"
export CFLAGS="${CFLAGS:-} ${file_map}"
export CXXFLAGS="${CXXFLAGS:-} ${file_map}"

echo
printf '\033[97m%s\033[0m\n' "Dile packages"
dim "repository  ${repo_root}"
dim "target      ${target_root}"
dim "tray        ${tray} appindicator"
dim "remapped    3 rustc path prefixes, and __FILE__ for the C and C++"

# ---------------------------------------------------------------------------------------
# 3. The sidecars, which have to exist before dile-app compiles at all.
# ---------------------------------------------------------------------------------------

if [ "${skip_sidecars}" -eq 1 ]; then
    dim "sidecars    skipped, crates/dile-app/binaries is assumed current"
else
    # `--cpu`, and on purpose: Linux does not probe the GPU tier and never chooses it for
    # anybody (docs/PROJECT.md §9, docs/BUILDING.md), so a Vulkan host in the package would be
    # a Vulkan toolchain in every release build for a tier nothing selects on its own. A person
    # who wants that tier builds the host themselves; docs/BUILDING.md says how.
    if ! "${script_directory}/build-host.sh" --cpu; then
        red "the sidecars did not build, so there is nothing to bundle."
        exit 1
    fi
fi

# ---------------------------------------------------------------------------------------
# 4. The bundle. deb and rpm, from crates/dile-app/tauri.linux.conf.json.
# ---------------------------------------------------------------------------------------

echo
dim "    cargo tauri build"
# From the repository root, for the same reason build-host.sh does it: the bundler resolves
# its configuration from the workspace it is standing in.
if ! (cd "${repo_root}" && cargo tauri build); then
    echo
    red "cargo tauri build failed."
    exit 1
fi

# ---------------------------------------------------------------------------------------
# 5. The check, and the deletion that makes it mean something.
# ---------------------------------------------------------------------------------------

echo
if ! "${script_directory}/check-binary-paths.sh"; then
    echo
    red "Deleting the bundle: a package that carries the build machine's paths"
    red "must not be left on disk looking finished."
    echo
    yellow "If the findings name ggml, the native library was built before this script set"
    yellow "CFLAGS, and cargo did not rebuild it. One command, then run this again:"
    yellow "  cargo clean -p transcribe-cpp-sys --release"
    rm -rf "${target_root}/release/bundle"
    exit 1
fi

# ---------------------------------------------------------------------------------------
# What docs/RELEASE.md asks to be kept.
# ---------------------------------------------------------------------------------------

echo
printf '\033[97m%s\033[0m\n' "Artefacts"
for name in dile-app dile-engine-host dile; do
    binary="${target_root}/release/${name}"
    if [ -f "${binary}" ]; then
        printf '  %-40s %10s\n' "${name}" "$(du -h "${binary}" | cut -f1)"
    fi
done

packages=()
while IFS= read -r package; do
    packages+=("${package}")
done < <(find "${target_root}/release/bundle" -maxdepth 2 -type f \( -name '*.deb' -o -name '*.rpm' \) 2>/dev/null | sort)

if [ ${#packages[@]} -eq 0 ]; then
    red "  no .deb or .rpm in ${target_root}/release/bundle"
    exit 1
fi

for package in "${packages[@]}"; do
    printf '  %-40s %10s\n' "$(basename "${package}")" "$(du -h "${package}" | cut -f1)"
    printf '    sha256  %s\n' "$(sha256sum "${package}" | cut -d' ' -f1)"
    printf '    at      %s\n' "${package}"
done

if [ "${tray}" = "legacy" ]; then
    echo
    yellow "--legacy-tray: these packages ask for libappindicator3-1, which is the 2018"
    yellow "library rather than the maintained one. This is not a release build."
fi

echo
printf '\033[32m%s\033[0m\n' "Green."
exit 0
