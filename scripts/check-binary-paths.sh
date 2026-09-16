#!/usr/bin/env bash
# Fails if a compiled binary carries the path of the machine that built it.
#
# `scripts/check-binary-paths.ps1` in shell spelling, and the same argument: every `panic!`,
# every `unwrap()` and every `#[track_caller]` location is compiled in as a **string literal**
# holding the source file it came from. `[profile.release] strip = true` does not touch those,
# and no grep over the working tree can see them — they exist only in the artefact. Measured
# on Windows before the fix existed, on the 0.1.0 tree: 159 copies of the maintainer's account
# name in `dile.exe` and 23 in `dile-engine-host.exe`.
#
# The same thing happens here and reads worse, because a Linux home directory is
# `/home/<account>` and the account name is usually a person's. `scripts/build-installer.sh`
# is what prevents it — `--remap-path-prefix` for the Rust half, `-ffile-prefix-map` for the C
# and C++ half — and this script is the proof that it is still working. The installer script
# runs it itself and **deletes the bundle** if it finds anything.
#
# One difference from the PowerShell original, and it is the reason this is not a translation:
# a Windows executable spells paths in three encodings, because its resource and manifest data
# is UTF-16. An ELF binary has no resource section; every string in it is UTF-8, so one pass
# over the bytes is the whole scan.
#
# Usage:
#   scripts/check-binary-paths.sh                 # the three release binaries
#   scripts/check-binary-paths.sh --profile debug # the same three, unremapped, as a control
#   scripts/check-binary-paths.sh path/to/binary [more...]
#
# Exit code 0 means every pattern counted zero. 1 is a finding, 2 is a missing file.

set -uo pipefail

profile="release"
files=()

while [ $# -gt 0 ]; do
    case "$1" in
        --profile | -Profile)
            profile="${2:-}"
            shift 2
            ;;
        --help | -h)
            sed -n '2,30p' "$0"
            exit 0
            ;;
        *)
            files+=("$1")
            shift
            ;;
    esac
done

if [ "$profile" != "release" ] && [ "$profile" != "debug" ]; then
    echo "--profile takes release or debug, not ${profile}" >&2
    exit 2
fi

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_directory}/.." && pwd)"
target_root="${CARGO_TARGET_DIR:-${repo_root}/target}"

if [ ${#files[@]} -eq 0 ]; then
    # The three programs the packages carry, which are the three that get published.
    files=(
        "${target_root}/${profile}/dile-app"
        "${target_root}/${profile}/dile-engine-host"
        "${target_root}/${profile}/dile"
    )
fi

# ---------------------------------------------------------------------------------------
# What must not be in a binary, and why each one is on the list.
#
# `/home/` rather than `/home/<account>`: a CI runner's `/home/runner` has to trip this too.
# It is not personal data there, but it is the same escape — if the runner's path survives the
# remap then so would a contributor's, and a pull request is where that should be found.
#
# `.rustup` and the Windows spellings are tripwires rather than fixes. Nothing on this
# toolchain produces them — the standard library arrives already remapped by the Rust project
# as `/rustc/<hash>/...` — so if one ever shows up, a prefix is missing and this goes red
# rather than quietly shipping.
#
# The absolute path of this checkout is on the list, which is what catches a source path from
# our own crates that escaped the remap. Deliberately not the bare string `dile/`: the
# remapped paths of this workspace's own crates read `crates/dile-app/src/main.rs`, which is
# relative, identical on every machine, and exactly what is wanted.
# ---------------------------------------------------------------------------------------

needles=(
    "/home/|a Linux home directory"
    "/Users/|a macOS home directory"
    '\Users\|a Windows user profile'
    ".cargo/registry|the cargo registry checkout"
    ".rustup|a rustup toolchain directory"
    "${repo_root}|this checkout"
)

findings=0

# A printable one-line window around a hit, so a finding names its own cause.
excerpt() {
    local file="$1" offset="$2"
    local from=$((offset - 16))
    [ "${from}" -lt 0 ] && from=0
    tail -c "+$((from + 1))" "${file}" |
        head -c 120 |
        tr -c '\40-\176' '.' |
        tr -s '.'
}

for file in "${files[@]}"; do
    if [ ! -f "${file}" ]; then
        echo "${file} is not there. Build it first."
        exit 2
    fi

    total=0
    breakdown=()
    examples=()

    for entry in "${needles[@]}"; do
        needle="${entry%%|*}"
        what="${entry#*|}"
        # `-a` because the file is binary, `-F` because a path is not a pattern, `-o` to count
        # matches rather than lines: a hundred copies on one line is a hundred findings.
        found="$(grep -a -o -F -- "${needle}" "${file}" 2>/dev/null | wc -l)"
        if [ "${found}" -gt 0 ]; then
            breakdown+=("$(printf '%5d  %s  (%s)' "${found}" "${needle}" "${what}")")
            total=$((total + found))
            if [ ${#examples[@]} -lt 5 ]; then
                while read -r hit; do
                    [ ${#examples[@]} -lt 5 ] || break
                    offset="${hit%%:*}"
                    examples+=("      @${offset} $(excerpt "${file}" "${offset}")")
                done < <(grep -a -b -o -F -- "${needle}" "${file}" 2>/dev/null | head -5)
            fi
        fi
    done

    findings=$((findings + total))
    size="$(du -h "${file}" | cut -f1)"
    printf '%-48s %10s  %d match(es)\n' "${file#"${repo_root}/"}" "${size}" "${total}"
    for line in "${breakdown[@]}"; do
        printf '    %s\n' "${line}"
    done
    for line in "${examples[@]}"; do
        printf '%s\n' "${line}"
    done
done

if [ "${findings}" -gt 0 ]; then
    echo
    echo "${findings} match(es) for a build-machine path in the binaries above."
    echo "Release binaries are published; these are the build machine's own directories, and"
    echo "\`strip\` does not remove them. scripts/build-installer.sh passes --remap-path-prefix"
    echo "and -ffile-prefix-map for exactly this — check that they still reach every call."
    exit 1
fi

echo "no machine-specific paths in any of them"
exit 0
