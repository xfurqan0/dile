#Requires -Version 5.1
<#
.SYNOPSIS
    Builds the sidecar binaries and puts them where the Tauri build looks for them.

.DESCRIPTION
    The bundle ships three programs. `Dile.exe` is the application; the other two are
    declared in `crates/dile-app/tauri.conf.json` under `bundle.externalBin`:

        dile-engine-host    the only binary that links transcribe-cpp
        dile                the command line, `dile transcribe <file>`

    Tauri picks those up by file name, under their target triple, from
    `crates/dile-app/binaries/`. The mechanism wants them on disk **before the application
    crate compiles**: `tauri-build` checks for them in the build script, so a missing sidecar
    fails `cargo clippy`, `cargo test` and `cargo tauri build --no-bundle` as well as the
    bundler. Cargo will not put them there on its own — they are separate crates — which is
    the whole reason this script exists.

    The name says "host" because the engine host is the one with a recipe worth writing down.
    The command line is built here too because the check above does not care which of the two
    is missing.

    Run it before anything that compiles `dile-app`:

        scripts\build-host.ps1                  release, with the GPU feature. What ships.
        scripts\build-host.ps1 -Cpu -DebugBuild cheap and SDK-free. What CI runs.

    The copies land in a git-ignored directory: they are build artefacts, and committing a
    binary to satisfy a build step is how a repository ends up shipping a stale one.

.PARAMETER Cpu
    Build the engine host without `gpu-vulkan`. No Vulkan SDK needed, about five minutes
    faster, and the result cannot run the Vulkan tier — it answers `hello` without the
    feature, which is what the application and `dile transcribe` both check. This is for CI,
    where the point is that the configuration is exercised rather than that the binary works.

.PARAMETER DebugBuild
    Build the debug profile instead of release. Named `-DebugBuild` rather than `-Debug`
    because `-Debug` is one of PowerShell's own common parameters and declaring it twice is
    an error at run time.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-host.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-host.ps1 -Cpu -DebugBuild
#>

[CmdletBinding()]
param(
    [switch]$Cpu,
    [switch]$DebugBuild
)

# Native tools report failure through their exit code, not through PowerShell's error stream.
$ErrorActionPreference = "Continue"

# Verb-Noun names only: a function named after a built-in alias silently loses to the alias.
function Write-Note {
    param([string]$Text)
    Write-Host $Text -ForegroundColor DarkGray
}

function Get-HostTriple {
    # The triple `bundle.externalBin` expects in the file name, from the compiler rather than
    # from a constant: a machine building for aarch64 would otherwise copy the file to a name
    # Tauri never looks at, and the failure would arrive as "sidecar not found" with a path
    # that is plainly there.
    $verbose = & rustc -vV
    if ($LASTEXITCODE -ne 0) {
        return $null
    }
    foreach ($line in $verbose) {
        if ($line.StartsWith("host: ")) {
            return $line.Substring(6).Trim()
        }
    }
    return $null
}

$scriptDirectory = $PSScriptRoot
if ([string]::IsNullOrEmpty($scriptDirectory)) {
    $scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
}
$repoRoot = Split-Path -Parent $scriptDirectory

$profileName = "release"
if ($DebugBuild) {
    $profileName = "debug"
}

# Honoured so that scripts\ci-local.ps1, which points it at a persistent cache, does not
# recompile transcribe.cpp on every run.
$targetRoot = $env:CARGO_TARGET_DIR
if ([string]::IsNullOrEmpty($targetRoot)) {
    $targetRoot = Join-Path $repoRoot "target"
}

$triple = Get-HostTriple
if ($null -eq $triple) {
    Write-Host "rustc -vV printed no host line, so the sidecar name cannot be worked out." -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "Dile sidecars" -ForegroundColor White
Write-Note ("repository  " + $repoRoot)
Write-Note ("profile     " + $profileName)
Write-Note ("triple      " + $triple)

# ---------------------------------------------------------------------------------------
# The one thing that breaks the GPU build, checked before five minutes are spent on it.
#
# The Vulkan SDK sets VULKAN_SDK machine-wide and puts glslc on PATH, so the shaders compile
# and everything looks fine until the very last step, where the linker cannot find
# vulkan-1.lib. LIB is the only missing piece and the installer does not set it.
# docs/BUILDING.md, "The GPU build, and the one thing that breaks it".
# ---------------------------------------------------------------------------------------

$features = @()
if ($Cpu) {
    Write-Note "features    (none) -- CPU only, this host cannot run the Vulkan tier"
} else {
    if ([string]::IsNullOrEmpty($env:VULKAN_SDK)) {
        Write-Host ""
        Write-Host "VULKAN_SDK is not set, so the GPU build would fail at the link step." -ForegroundColor Red
        Write-Host "Install the Vulkan SDK (1.3 or newer) and open a new shell, or pass -Cpu" -ForegroundColor Yellow
        Write-Host "to build a host that cannot run the Vulkan tier." -ForegroundColor Yellow
        exit 1
    }
    $vulkanLib = Join-Path $env:VULKAN_SDK "Lib"
    if (-not (Test-Path -LiteralPath (Join-Path $vulkanLib "vulkan-1.lib"))) {
        Write-Host ""
        Write-Host ("vulkan-1.lib is not in " + $vulkanLib + ", so VULKAN_SDK points at an incomplete SDK.") -ForegroundColor Red
        exit 1
    }
    $env:LIB = $vulkanLib + ";" + $env:LIB
    $features = @("--features", "gpu-vulkan")
    Write-Note ("features    gpu-vulkan -- SDK " + $env:VULKAN_SDK)
}

# ---------------------------------------------------------------------------------------
# Build, then copy. Two crates, one loop, because the only difference between them is the
# feature list and the engine host is the only one that has any.
# ---------------------------------------------------------------------------------------

$binariesDirectory = Join-Path $repoRoot "crates\dile-app\binaries"
New-Item -ItemType Directory -Force -Path $binariesDirectory | Out-Null

$sidecars = @(
    @{ Package = "dile-engine-host"; Stem = "dile-engine-host"; Features = $features },
    @{ Package = "dile-cli"; Stem = "dile"; Features = @() }
)

# Cargo is run from the repository root rather than from wherever the caller happened to be:
# `cargo build -p ...` resolves the workspace from the current directory, and a script that
# only works when it is invoked from one place is a script that fails on release day.
$copied = New-Object System.Collections.ArrayList
Push-Location $repoRoot
try {
foreach ($sidecar in $sidecars) {
    $arguments = @("build", "-p", $sidecar.Package)
    if (-not $DebugBuild) {
        $arguments += "--release"
    }
    $arguments += $sidecar.Features

    Write-Host ""
    Write-Note ("    cargo " + ($arguments -join " "))
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) {
        Write-Host ""
        Write-Host ("cargo build -p " + $sidecar.Package + " failed.") -ForegroundColor Red
        exit 1
    }

    $source = Join-Path $targetRoot ($profileName + "\" + $sidecar.Stem + ".exe")
    if (-not (Test-Path -LiteralPath $source)) {
        Write-Host ("cargo did not produce " + $source) -ForegroundColor Red
        exit 1
    }
    $destination = Join-Path $binariesDirectory ($sidecar.Stem + "-" + $triple + ".exe")
    Copy-Item -LiteralPath $source -Destination $destination -Force

    $size = (Get-Item -LiteralPath $destination).Length
    [void]$copied.Add([pscustomobject]@{
        Name = Split-Path $destination -Leaf
        Megabytes = [math]::Round($size / 1MB, 2)
    })
}
} finally {
    Pop-Location
}

Write-Host ""
Write-Host "Ready for cargo tauri build" -ForegroundColor Green
foreach ($row in $copied) {
    Write-Host ("  " + $row.Name.PadRight(48) + ($row.Megabytes.ToString("0.00", [System.Globalization.CultureInfo]::InvariantCulture)).PadLeft(8) + " MB")
}
Write-Note ("  in " + $binariesDirectory)
exit 0
