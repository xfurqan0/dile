#Requires -Version 5.1
<#
.SYNOPSIS
    Fails if a compiled binary carries the path of the machine that built it.

.DESCRIPTION
    Every `panic!`, every `unwrap()` and every `#[track_caller]` location is compiled in as a
    **string literal** holding the source file it came from. `[profile.release] strip = true`
    does not touch those, and no grep over the working tree can see them: they exist only in
    the artefact.

    Measured on this repository before the fix existed, on the 0.1.0 tree:

        dile.exe                159 copies of C:\Users\<account>\.cargo\registry\src\...
        dile-engine-host.exe     23 copies of the same

    That is the maintainer's account name inside an installer published for anyone to
    download. The fix is `--remap-path-prefix`, passed by `scripts/build-installer.ps1` to
    every cargo call the installer build makes; this script is the proof that it is still
    working, and `build-installer.ps1` runs it itself before it will leave an installer on
    disk.

    `trim-paths = "all"` in the release profile would be the tidy way and is not available:
    still nightly-only on the toolchain `rust-toolchain.toml` pins.

    Exit code 0 means every pattern counted zero. Anything else is a finding, printed with
    the first five in context so the next question — *which* prefix escaped — is already
    answered.

.PARAMETER Profile
    Which profile's binaries to read: `release` (the default) or `debug`.

.PARAMETER Path
    Binaries to read instead of the three the bundle ships.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-binary-paths.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-binary-paths.ps1 -Profile debug
#>

[CmdletBinding()]
param(
    [ValidateSet("release", "debug")]
    [string]$Profile = "release",
    [string[]]$Path
)

$ErrorActionPreference = "Stop"

# Verb-Noun names only: a function named after a built-in alias silently loses to the alias.
function Format-Bytes {
    # Invariant culture on purpose: a Turkish or German console would otherwise print 2,38 MB
    # and a table pasted into an issue would read differently on every machine.
    param([long]$Value)
    $invariant = [System.Globalization.CultureInfo]::InvariantCulture
    if ($Value -lt 1024) { return ([string]$Value + " B") }
    if ($Value -lt 1048576) { return (($Value / 1024).ToString("0.0", $invariant) + " KB") }
    return (($Value / 1048576).ToString("0.00", $invariant) + " MB")
}

function Get-Excerpt {
    # A printable one-line window around a hit, so the finding names its own cause.
    param([string]$Text, [int]$At, [int]$Length)
    $from = [math]::Max(0, $At - 16)
    $to = [math]::Min($Text.Length, $At + $Length + 64)
    $slice = $Text.Substring($from, $to - $from)
    return ($slice -replace '[^\x20-\x7e]+', '.')
}

$scriptDirectory = $PSScriptRoot
if ([string]::IsNullOrEmpty($scriptDirectory)) {
    $scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
}
$repoRoot = (Resolve-Path (Split-Path -Parent $scriptDirectory)).Path.TrimEnd('\')

# Honoured for the same reason the build scripts honour it: a comparison build in a second
# target directory has to be checkable too.
$targetRoot = $env:CARGO_TARGET_DIR
if ([string]::IsNullOrEmpty($targetRoot)) {
    $targetRoot = Join-Path $repoRoot "target"
}

$files = $Path
if ($null -eq $files -or $files.Count -eq 0) {
    # The three programs the installer carries, which are the three that get published.
    $files = @(
        (Join-Path $targetRoot ($Profile + "\dile-app.exe")),
        (Join-Path $targetRoot ($Profile + "\dile-engine-host.exe")),
        (Join-Path $targetRoot ($Profile + "\dile.exe"))
    )
}

# ---------------------------------------------------------------------------------------
# What must not be in a binary, and why each one is on the list.
#
# Matched case-insensitively, because Windows spells the same directory several ways and a
# check that only knows one of them is a check that can be walked past.
#
# `\Users\` rather than `C:\Users\`: the drive letter is not the interesting part, and a
# GitHub runner's `C:\Users\runneradmin` has to trip this too. It is not personal data
# there, but it is the same escape -- if the runner's path survives the remap then so would
# a contributor's, and the pull request is where that should be found.
#
# `.rustup` is a tripwire rather than a fix. The standard library's own paths already arrive
# remapped by the Rust project as `/rustc/<hash>/library/...`, so nothing on this toolchain
# produces it and `build-installer.ps1` has no prefix for it. If it ever shows up -- a
# toolchain built from source, `-Zbuild-std` -- this goes red and a fourth prefix is the
# answer.
#
# The absolute path of this checkout is on the list in both spellings, which is what catches
# a source path from our own crates that escaped the remap. Deliberately not the bare string
# `dile\`: the remapped paths of this workspace's own crates read `crates\dile-app\src\
# main.rs`, which is relative, identical on every machine, and exactly what is wanted.
# ---------------------------------------------------------------------------------------

$patterns = @(
    @{ Needle = "\Users\"; What = "a Windows user profile" },
    @{ Needle = "/Users/"; What = "a macOS home directory" },
    @{ Needle = "/home/"; What = "a Linux home directory" },
    @{ Needle = ".cargo\registry"; What = "the cargo registry checkout" },
    @{ Needle = ".cargo/registry"; What = "the cargo registry checkout" },
    @{ Needle = ".rustup"; What = "a rustup toolchain directory" },
    @{ Needle = $repoRoot; What = "this checkout" },
    @{ Needle = ($repoRoot -replace '\\', '/'); What = "this checkout" }
) | Group-Object -Property { $_.Needle } | ForEach-Object { $_.Group[0] }

$findings = 0

foreach ($file in $files) {
    if (-not (Test-Path -LiteralPath $file)) {
        Write-Host ($file + " is not there. Build it first.") -ForegroundColor Red
        exit 2
    }

    $bytes = [System.IO.File]::ReadAllBytes($file)

    # The three ways a path can be spelled inside a Windows executable. Rust compiles its
    # source locations as UTF-8, which latin-1 reads back byte for byte, and that is where
    # every one of the 182 original findings was. Resource and manifest data is UTF-16 and is
    # not aligned to any particular offset, hence the second decode from one byte in, which is
    # what makes an odd-offset wide string visible at all.
    $views = @(
        @{ Encoding = "text"; Text = [System.Text.Encoding]::GetEncoding(28591).GetString($bytes); Stride = 1; Skew = 0 },
        @{ Encoding = "utf-16"; Text = [System.Text.Encoding]::Unicode.GetString($bytes); Stride = 2; Skew = 0 },
        @{ Encoding = "utf-16"; Text = [System.Text.Encoding]::Unicode.GetString($bytes, 1, $bytes.Length - 1); Stride = 2; Skew = 1 }
    )

    $counts = New-Object System.Collections.Specialized.OrderedDictionary
    $examples = New-Object System.Collections.ArrayList
    $total = 0

    foreach ($pattern in $patterns) {
        $needle = $pattern.Needle
        $found = 0
        foreach ($view in $views) {
            $text = $view.Text
            $at = $text.IndexOf($needle, [System.StringComparison]::OrdinalIgnoreCase)
            while ($at -ne -1) {
                $found += 1
                if ($examples.Count -lt 5) {
                    [void]$examples.Add([pscustomobject]@{
                        Offset = $at * $view.Stride + $view.Skew
                        Encoding = $view.Encoding
                        Text = Get-Excerpt -Text $text -At $at -Length $needle.Length
                    })
                }
                $at = $text.IndexOf($needle, $at + 1, [System.StringComparison]::OrdinalIgnoreCase)
            }
        }
        if ($found -gt 0) {
            $counts[($needle + "  (" + $pattern.What + ")")] = $found
            $total += $found
        }
    }

    $findings += $total

    # Matches, not strings: one `C:\Users\...\.cargo\registry\...` answers to two of the
    # patterns above, and calling that two leaks would be a number nobody could reconcile
    # with the breakdown printed under it.
    $name = (Resolve-Path -LiteralPath $file).Path.Replace($repoRoot + "\", "")
    $size = Format-Bytes -Value (Get-Item -LiteralPath $file).Length
    Write-Host ($name.PadRight(48) + $size.PadLeft(12) + "  " + $total + " match(es)")

    foreach ($label in $counts.Keys) {
        Write-Host ("    " + ([string]$counts[$label]).PadLeft(5) + "  " + $label)
    }
    foreach ($example in $examples) {
        Write-Host ("      @" + $example.Offset + " " + $example.Encoding + ": " + $example.Text)
    }
}

if ($findings -gt 0) {
    Write-Host ""
    Write-Host ("" + $findings + " match(es) for a build-machine path in the binaries above.") -ForegroundColor Red
    Write-Host "Release binaries are published; these are the build machine's own directories," -ForegroundColor Yellow
    Write-Host "and `strip` does not remove them. scripts\build-installer.ps1 passes" -ForegroundColor Yellow
    Write-Host "--remap-path-prefix for exactly this -- check that it still reaches every cargo call." -ForegroundColor Yellow
    exit 1
}

Write-Host "no machine-specific paths in any of them" -ForegroundColor Green
exit 0
