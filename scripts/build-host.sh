#!/usr/bin/env bash
#
# Builds the sidecar binaries and puts them where the Tauri build looks for them.
#
# The Linux half of scripts/build-host.ps1. Same job, same order, same output names minus the
# `.exe`: the bundle ships three programs, and two of them are declared in
# `crates/dile-app/tauri.conf.json` under `bundle.externalBin`:
#
#     dile-engine-host    the only binary that links transcribe-cpp
#     dile                the command line, `dile transcribe <file>`
#
# Tauri picks those up by file name, under their target triple, from
# `crates/dile-app/binaries/`. The mechanism wants them on disk **before the application crate
# compiles**: `tauri-build` checks for them in its build script, so a missing sidecar fails
# `cargo clippy`, `cargo test` and `cargo tauri build --no-bundle` as well as the bundler.
# Cargo will not put them there on its own — they are separate crates — which is the whole
# reason this script exists.
#
# Run it before anything that compiles `dile-app`:
#
#     scripts/build-host.sh                 release, with the GPU feature. What ships.
#     scripts/build-host.sh --cpu --debug   cheap and Vulkan-free. What CI runs.
#
# The switches are the PowerShell script's in shell spelling: `--cpu` is `-Cpu` and `--debug`
# is `-DebugBuild`. Both PowerShell spellings are accepted too, so a reader following
# docs/BUILDING.md's Windows column on a Linux machine is not stopped by a dash.
#
#   --cpu     Build the engine host without `gpu-vulkan`. No Vulkan headers and no glslc
#             needed, several minutes faster, and the result cannot run the Vulkan tier — it
#             answers `hello` without the feature, which is what the application and
#             `dile transcribe` both check. This is what CI runs, where the point is that the
#             configuration is exercised rather than that the binary works. It is also the
#             sane default on this platform for a second reason: Vulkan on Intel integrated
#             graphics has an open `DeviceLost` fault in whisper.cpp, so the CPU tier is the
#             one Linux trusts (docs/PROJECT.md §3, engine tiers).
#
#   --debug   Build the debug profile instead of release.
#
# The copies land in a git-ignored directory: they are build artefacts, and committing a
# binary to satisfy a build step is how a repository ends up shipping a stale one.

set -u -o pipefail

cpu=0
debug_build=0

for argument in "$@"; do
    case "$argument" in
        --cpu | -Cpu | -cpu) cpu=1 ;;
        --debug | -DebugBuild | -debugbuild) debug_build=1 ;;
        -h | --help)
            sed -n '3,45p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown switch: $argument" >&2
            echo "usage: scripts/build-host.sh [--cpu] [--debug]" >&2
            exit 2
            ;;
    esac
done

note() { printf '\033[90m%s\033[0m\n' "$1"; }
bad() { printf '\033[31m%s\033[0m\n' "$1" >&2; }
hint() { printf '\033[33m%s\033[0m\n' "$1" >&2; }

script_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(dirname -- "$script_directory")"

profile_name="release"
if [ "$debug_build" -eq 1 ]; then
    profile_name="debug"
fi

# Honoured so that a caller pointing it at a persistent cache does not recompile
# transcribe.cpp on every run — the same trade the CI job makes with rust-cache.
target_root="${CARGO_TARGET_DIR:-$repository_root/target}"

# The triple `bundle.externalBin` expects in the file name, from the compiler rather than from
# a constant: a machine building for aarch64 would otherwise copy the file to a name Tauri
# never looks at, and the failure would arrive as "sidecar not found" with a path that is
# plainly there.
triple="$(rustc -vV | sed -n 's/^host: //p')"
if [ -z "$triple" ]; then
    bad "rustc -vV printed no host line, so the sidecar name cannot be worked out."
    exit 1
fi

echo
printf '\033[97m%s\033[0m\n' "Dile sidecars"
note "repository  $repository_root"
note "profile     $profile_name"
note "triple      $triple"

# ---------------------------------------------------------------------------------------
# The one thing that breaks the GPU build, checked before the minutes are spent on it.
#
# On Windows the trap is `LIB`, which the SDK installer does not set. There is no SDK here:
# the loader, the headers and glslc are three distribution packages, and CMake's FindVulkan
# fails with all three names at once — `Could NOT find Vulkan (missing: Vulkan_LIBRARY
# Vulkan_INCLUDE_DIR glslc)` — which is a clearer message than the linker's but arrives just
# as late. So the same check, with the same purpose, against pkg-config and PATH.
# ---------------------------------------------------------------------------------------

features=()
if [ "$cpu" -eq 1 ]; then
    note "features    (none) -- CPU only, this host cannot run the Vulkan tier"
else
    missing=()
    pkg-config --exists vulkan 2>/dev/null || missing+=("the Vulkan loader and headers")
    command -v glslc >/dev/null 2>&1 || missing+=("glslc, the shader compiler")
    if [ "${#missing[@]}" -ne 0 ]; then
        echo
        bad "The GPU build cannot start: CMake would not find ${missing[*]}."
        hint "Fedora:  sudo dnf install vulkan-loader-devel vulkan-headers glslc"
        hint "Debian:  sudo apt install libvulkan-dev glslc"
        hint "Or pass --cpu to build a host that cannot run the Vulkan tier."
        exit 1
    fi
    features=(--features gpu-vulkan)
    note "features    gpu-vulkan -- $(pkg-config --modversion vulkan) loader, $(command -v glslc)"
fi

# ---------------------------------------------------------------------------------------
# Build, then copy. Two crates, one loop, because the only difference between them is the
# feature list and the engine host is the only one that has any.
# ---------------------------------------------------------------------------------------

binaries_directory="$repository_root/crates/dile-app/binaries"
mkdir -p "$binaries_directory"

# package:stem, in the order the PowerShell script builds them.
sidecars=("dile-engine-host:dile-engine-host" "dile-cli:dile")

# Cargo is run from the repository root rather than from wherever the caller happened to be:
# `cargo build -p ...` resolves the workspace from the current directory, and a script that
# only works when it is invoked from one place is a script that fails on release day.
cd "$repository_root" || exit 1

copied=()
for sidecar in "${sidecars[@]}"; do
    package="${sidecar%%:*}"
    stem="${sidecar##*:}"

    arguments=(build -p "$package")
    if [ "$debug_build" -eq 0 ]; then
        arguments+=(--release)
    fi
    if [ "$package" = "dile-engine-host" ] && [ "${#features[@]}" -ne 0 ]; then
        arguments+=("${features[@]}")
    fi

    echo
    note "    cargo ${arguments[*]}"
    if ! cargo "${arguments[@]}"; then
        echo
        bad "cargo build -p $package failed."
        exit 1
    fi

    source_path="$target_root/$profile_name/$stem"
    if [ ! -f "$source_path" ]; then
        bad "cargo did not produce $source_path"
        exit 1
    fi
    destination="$binaries_directory/$stem-$triple"
    cp -f "$source_path" "$destination"

    # `LC_ALL=C` because the size is formatted, not computed: a shell in a locale whose
    # decimal separator is a comma prints `68,85` and then refuses to read its own output back.
    bytes="$(stat -c %s "$destination")"
    copied+=("$(LC_ALL=C awk -v name="$stem-$triple" -v bytes="$bytes" \
        'BEGIN { printf "%-48s %8.2f MB", name, bytes / 1048576 }')")
done

echo
printf '\033[32m%s\033[0m\n' "Ready for cargo tauri build"
for row in "${copied[@]}"; do
    echo "  $row"
done
note "  in $binaries_directory"
exit 0
