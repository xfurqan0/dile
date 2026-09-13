#Requires -Version 5.1
<#
.SYNOPSIS
    Builds the installer the way a release is built, and refuses to leave a bad one on disk.

.DESCRIPTION
    Four things whose order matters, which is why this is one script and not four steps:

        1. the remapped environment, so no cargo call below can compile a machine path in
        2. the sidecars (scripts\build-host.ps1) -- the engine host with the GPU feature, and
           the command line -- because `tauri-build` checks for them before dile-app compiles
        3. `cargo tauri build`, which bundles NSIS
        4. `scripts\check-binary-paths.ps1` over all three programs

    Step 4 is not a courtesy. Panic locations are compiled in as string literals, `strip`
    does not remove them, and a release build can therefore carry the path every crate was
    compiled from -- an account name, published. If the check finds one, **the bundle is
    deleted** rather than left on disk looking finished, so an installer that exists after
    this script ran is one that passed.

    What it prints at the end is what `docs/RELEASE.md` step 2 asks to be kept: the three
    binaries and the installer, with their sizes and the installer's SHA-256.

.PARAMETER Cpu
    Build the engine host without `gpu-vulkan`. For trying the packaging on a machine with no
    Vulkan SDK; **never for a release**, because the installer would carry a host that cannot
    run the tier this product defaults to. The script says so at the end.

.PARAMETER SkipSidecars
    Assume `crates\dile-app\binaries\` is already current. For a second run that only changes
    the application, where rebuilding a 55 MB engine host is five minutes for nothing.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-installer.ps1
#>

[CmdletBinding()]
param(
    [switch]$Cpu,
    [switch]$SkipSidecars
)

$ErrorActionPreference = "Continue"

# Verb-Noun names only: a function named after a built-in alias silently loses to the alias.
function Write-Note {
    param([string]$Text)
    Write-Host $Text -ForegroundColor DarkGray
}

$scriptDirectory = $PSScriptRoot
if ([string]::IsNullOrEmpty($scriptDirectory)) {
    $scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
}
$repoRoot = (Resolve-Path (Split-Path -Parent $scriptDirectory)).Path.TrimEnd('\')

$targetRoot = $env:CARGO_TARGET_DIR
if ([string]::IsNullOrEmpty($targetRoot)) {
    $targetRoot = Join-Path $repoRoot "target"
}

# ---------------------------------------------------------------------------------------
# 1. The remapped environment.
#
# Three prefixes, which is every one that shows up in practice on this toolchain. The
# standard library arrives already remapped by the Rust project as `/rustc/<hash>/...`, and
# nothing here builds it from source; `.rustup` is a pattern in the checker rather than a
# prefix here, so a toolchain that ever did would go red instead of quietly shipping.
#
# The registry maps to `cargo` rather than to `crates`, because this workspace keeps its own
# code in `crates/` and a dependency panicking from `crates\serde_json-1.0.x\src\...` would
# read like one of ours.
#
# CARGO_ENCODED_RUSTFLAGS and not RUSTFLAGS: the unencoded variable is split on whitespace,
# and a Windows home directory with a space in it would break every flag above in a way that
# looks like a compiler bug. Cargo reads the encoded one in preference and ignores the other
# entirely when it is set, so whatever the caller asked for is carried across rather than
# replaced.
# ---------------------------------------------------------------------------------------

$cargoHome = $env:CARGO_HOME
if ([string]::IsNullOrEmpty($cargoHome)) {
    $cargoHome = Join-Path $env:USERPROFILE ".cargo"
}

$separator = [string][char]0x1f
$inherited = @()
if (-not [string]::IsNullOrEmpty($env:CARGO_ENCODED_RUSTFLAGS)) {
    $inherited = $env:CARGO_ENCODED_RUSTFLAGS.Split([char]0x1f)
} elseif (-not [string]::IsNullOrEmpty($env:RUSTFLAGS)) {
    $inherited = $env:RUSTFLAGS -split '\s+'
}
$remaps = @(
    ("--remap-path-prefix=" + (Join-Path $cargoHome "registry\src") + "=cargo"),
    ("--remap-path-prefix=" + (Join-Path $cargoHome "git\checkouts") + "=git"),
    ("--remap-path-prefix=" + $repoRoot + "=dile")
)
$env:CARGO_ENCODED_RUSTFLAGS = ((@($inherited | Where-Object { $_ }) + $remaps) -join $separator)

# --------------------------------------------------------------------------------------
# And the same thing for the C and C++ half, which --remap-path-prefix cannot reach.
#
# `transcribe-cpp-sys` compiles ggml from source through CMake, and every GGML_ASSERT and
# GGML_ABORT in it carries `__FILE__`. Measured on the 0.1.0 tree: 17 copies of
# `C:\Users\<account>\.cargo\registry\src\...\transcribe-cpp-sys-0.2.3\ggml\src\ggml.c` and
# its neighbours, inside `dile-engine-host.exe`, surviving `strip` and every rustc flag
# above. `/d1trimfile:` is MSVC's answer -- undocumented, and the only one there is: it trims
# the prefix from `__FILE__` at compile time, so the same assert reads `ggml\src\ggml.c`.
#
# The cmake crate appends CFLAGS and CXXFLAGS to what it derives itself, so this adds rather
# than replaces, and the sys crate's own /O2 /Ob2 /DNDEBUG are untouched.
#
# **Cargo does not rebuild on a CFLAGS change**, because the sys crate declares no
# `rerun-if-env-changed` for it. A tree that already built the native library without this
# keeps the library it has, and `check-binary-paths.ps1` at the end is what notices. Its
# failure message names the one command that fixes it.
# --------------------------------------------------------------------------------------

$trim = "/d1trimfile:" + (Join-Path $cargoHome "registry\src") + "\"
$env:CFLAGS = (($env:CFLAGS, $trim) | Where-Object { $_ }) -join " "
$env:CXXFLAGS = (($env:CXXFLAGS, $trim) | Where-Object { $_ }) -join " "

Write-Host ""
Write-Host "Dile installer" -ForegroundColor White
Write-Note ("repository  " + $repoRoot)
Write-Note ("target      " + $targetRoot)
Write-Note ("remapped    " + ($remaps.Count) + " rustc path prefixes, and __FILE__ for the C and C++")

# ---------------------------------------------------------------------------------------
# 2. The sidecars, which have to exist before dile-app compiles at all.
# ---------------------------------------------------------------------------------------

if ($SkipSidecars) {
    Write-Note "sidecars    skipped, crates\dile-app\binaries is assumed current"
} else {
    $sidecarArguments = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $scriptDirectory "build-host.ps1"))
    if ($Cpu) {
        $sidecarArguments += "-Cpu"
    }
    & powershell @sidecarArguments
    if ($LASTEXITCODE -ne 0) {
        Write-Host "the sidecars did not build, so there is nothing to bundle." -ForegroundColor Red
        exit 1
    }
}

# ---------------------------------------------------------------------------------------
# 3. The bundle. NSIS only, per user -- crates\dile-app\tauri.conf.json.
# ---------------------------------------------------------------------------------------

Write-Host ""
Write-Note "    cargo tauri build"
# From the repository root, for the same reason build-host.ps1 does it: the bundler resolves
# its configuration from the workspace it is standing in.
Push-Location $repoRoot
try {
    & cargo tauri build
} finally {
    Pop-Location
}
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "cargo tauri build failed." -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------------------------------------
# 4. The check, and the deletion that makes it mean something.
# ---------------------------------------------------------------------------------------

Write-Host ""
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $scriptDirectory "check-binary-paths.ps1")
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "Deleting the bundle: an installer that carries the build machine's paths" -ForegroundColor Red
    Write-Host "must not be left on disk looking finished." -ForegroundColor Red
    Write-Host ""
    Write-Host "If the findings name ggml, the native library was built before this script set" -ForegroundColor Yellow
    Write-Host "CFLAGS, and cargo did not rebuild it. One command, then run this again:" -ForegroundColor Yellow
    Write-Host "  cargo clean -p transcribe-cpp-sys --release" -ForegroundColor Yellow
    Remove-Item -Recurse -Force (Join-Path $targetRoot "release\bundle") -ErrorAction SilentlyContinue
    exit 1
}

# ---------------------------------------------------------------------------------------
# What docs/RELEASE.md step 2 asks to be kept.
# ---------------------------------------------------------------------------------------

Write-Host ""
Write-Host "Artefacts" -ForegroundColor White
foreach ($name in @("dile-app.exe", "dile-engine-host.exe", "dile.exe")) {
    $binary = Join-Path $targetRoot ("release\" + $name)
    if (Test-Path -LiteralPath $binary) {
        $megabytes = (Get-Item -LiteralPath $binary).Length / 1MB
        Write-Host ("  " + $name.PadRight(40) + ($megabytes.ToString("0.00", [System.Globalization.CultureInfo]::InvariantCulture)).PadLeft(10) + " MB")
    }
}

$installers = Get-ChildItem -Path (Join-Path $targetRoot "release\bundle\nsis") -Filter "*-setup.exe" -ErrorAction SilentlyContinue
if ($null -eq $installers -or $installers.Count -eq 0) {
    Write-Host "  no installer in target\release\bundle\nsis" -ForegroundColor Red
    exit 1
}
foreach ($installer in $installers) {
    $megabytes = $installer.Length / 1MB
    Write-Host ("  " + $installer.Name.PadRight(40) + ($megabytes.ToString("0.00", [System.Globalization.CultureInfo]::InvariantCulture)).PadLeft(10) + " MB")
    Write-Host ("    sha256  " + (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash.ToLower())
    Write-Host ("    at      " + $installer.FullName)
}

if ($Cpu) {
    Write-Host ""
    Write-Host "-Cpu: the engine host in this installer cannot run the Vulkan tier." -ForegroundColor Yellow
    Write-Host "This is not a release build. docs/RELEASE.md step 2." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "Green." -ForegroundColor Green
exit 0
