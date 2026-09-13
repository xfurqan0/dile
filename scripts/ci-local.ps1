#Requires -Version 5.1
<#
.SYNOPSIS
    Runs the CI gate on this machine, against a fresh clone of HEAD.

.DESCRIPTION
    A local mirror of .github/workflows/ci.yml. It runs the same steps, in the same order,
    with the same environment as the `windows` job, then the single step of the `audit` job:

        cmake --version
        scripts\build-host.ps1 -Cpu -DebugBuild
        cargo fmt --all -- --check
        cargo clippy --workspace --all-targets -- -D warnings
        cargo test --workspace
        cargo tauri build --debug --no-bundle
        cargo deny check

    The point of the script is the clone. Running the steps in the working tree proves less
    than it looks: an untracked file, or one that .gitignore hides, can carry the piece that
    makes a broken commit pass. So HEAD is cloned into a fresh temporary directory and
    everything runs there, which is what a runner gets from actions/checkout.

    The build cache is deliberately NOT fresh. CARGO_TARGET_DIR points at a persistent
    directory under LOCALAPPDATA, because transcribe-cpp-sys compiles transcribe.cpp through
    CMake and that is two to three minutes cold and nothing warm. This is the same trade-off
    the CI job makes with Swatinem/rust-cache: the source tree is fresh, the build cache is
    warm. Use -Clean when a stale artefact is the suspect.

    The temporary clone is removed when every step passes, and kept when one fails so the
    failure can be reproduced by hand. The path is printed either way.

.PARAMETER SkipTauriBuild
    Skips `cargo tauri build`, leaving fmt, clippy, test and cargo deny. This is the quick
    loop, and it is what the pre-push hook from scripts/install-hooks.ps1 runs.

.PARAMETER AllowDirty
    Runs even when tracked files have uncommitted changes. The clone still comes from HEAD,
    so those changes are NOT tested; only the committed state is. The script says so on the
    way in.

.PARAMETER Clean
    Wipes the persistent build cache before running, so everything is compiled from scratch.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ci-local.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ci-local.ps1 -SkipTauriBuild
#>

[CmdletBinding()]
param(
    [switch]$SkipTauriBuild,
    [switch]$AllowDirty,
    [switch]$Clean
)

# Native tools report failure through their exit code, not through PowerShell's error stream,
# so every step checks $LASTEXITCODE by hand. Nothing below relies on exceptions.
$ErrorActionPreference = "Continue"

# ---------------------------------------------------------------------------------------
# Helpers. Verb-Noun names only: a function named after a built-in alias (rm, cp, ls, sort,
# select) silently loses to the alias, and the failure is invisible until something deletes
# the wrong thing.
# ---------------------------------------------------------------------------------------

function Write-Banner {
    param([string]$Text)
    Write-Host ""
    Write-Host ("=== " + $Text) -ForegroundColor Cyan
}

function Write-Note {
    param([string]$Text)
    Write-Host $Text -ForegroundColor DarkGray
}

function Test-ToolPresence {
    # $ProbeArguments empty means presence on PATH is enough. Otherwise the tool is run and
    # its exit code decides, which is how a cargo subcommand (cargo tauri, cargo deny) has to
    # be tested: cargo itself is always on PATH.
    param(
        [string]$Executable,
        [string[]]$ProbeArguments
    )
    $onPath = Get-Command $Executable -ErrorAction SilentlyContinue
    if ($null -eq $onPath) {
        return $false
    }
    if ($null -eq $ProbeArguments) {
        return $true
    }
    if ($ProbeArguments.Count -eq 0) {
        return $true
    }
    & $Executable @ProbeArguments > $null 2> $null
    return ($LASTEXITCODE -eq 0)
}

function Remove-Directory {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return $true
    }
    try {
        Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
        return $true
    } catch {
        # Git writes its pack files read-only, which stops Remove-Item on some setups. Clear
        # the attribute across the tree and try once more before giving up.
        try {
            Get-ChildItem -LiteralPath $Path -Recurse -Force -File -ErrorAction Stop |
                ForEach-Object { $_.IsReadOnly = $false }
            Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
            return $true
        } catch {
            return $false
        }
    }
}

function Invoke-CiStep {
    param(
        [string]$Name,
        [string]$Executable,
        [string[]]$StepArguments
    )
    Write-Banner $Name
    Write-Note ("    " + $Executable + " " + ($StepArguments -join " "))
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    & $Executable @StepArguments
    $code = $LASTEXITCODE
    $watch.Stop()
    $status = "ok"
    if ($code -ne 0) {
        $status = "FAILED"
    }
    return [pscustomobject]@{
        Step     = $Name
        Status   = $status
        Seconds  = [math]::Round($watch.Elapsed.TotalSeconds, 1)
        ExitCode = $code
    }
}

function Format-Seconds {
    # Invariant culture on purpose: a Turkish or German console would otherwise print 155,0
    # and a table pasted into an issue would read differently on every machine.
    param([double]$Value)
    return $Value.ToString("0.0", [System.Globalization.CultureInfo]::InvariantCulture)
}

function Write-Summary {
    param([object[]]$Rows)
    $stepWidth = 4
    foreach ($row in $Rows) {
        if ($row.Step.Length -gt $stepWidth) {
            $stepWidth = $row.Step.Length
        }
    }
    Write-Host ""
    Write-Host ("  " + "Step".PadRight($stepWidth) + "  " + "Status".PadRight(8) + "  " + "Seconds".PadLeft(8))
    Write-Host ("  " + ("-" * $stepWidth) + "  " + ("-" * 8) + "  " + ("-" * 8))
    foreach ($row in $Rows) {
        $colour = "Green"
        if ($row.Status -eq "FAILED") {
            $colour = "Red"
        }
        if ($row.Status -eq "skipped") {
            $colour = "DarkGray"
        }
        $seconds = ""
        if ($row.Status -ne "skipped") {
            $seconds = Format-Seconds -Value $row.Seconds
        }
        Write-Host ("  " + $row.Step.PadRight($stepWidth) + "  " + $row.Status.PadRight(8) + "  " + $seconds.PadLeft(8)) -ForegroundColor $colour
    }
}

# ---------------------------------------------------------------------------------------
# Where we are.
# ---------------------------------------------------------------------------------------

$scriptDirectory = $PSScriptRoot
if ([string]::IsNullOrEmpty($scriptDirectory)) {
    $scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
}
$repoRoot = Split-Path -Parent $scriptDirectory

& git -C $repoRoot rev-parse --is-inside-work-tree > $null 2> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host ("Not a git work tree: " + $repoRoot) -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "Dile local CI gate" -ForegroundColor White
Write-Note ("repository  " + $repoRoot)

# ---------------------------------------------------------------------------------------
# Refuse to pretend. A dirty tree cannot be tested by cloning HEAD, so either it is clean or
# the caller is told, in as many words, that their edits are not in this run.
# ---------------------------------------------------------------------------------------

$dirty = & git -C $repoRoot status --porcelain --untracked-files=no
if ($LASTEXITCODE -ne 0) {
    Write-Host "git status failed." -ForegroundColor Red
    exit 1
}

if (-not [string]::IsNullOrWhiteSpace(($dirty -join ""))) {
    if (-not $AllowDirty) {
        Write-Host ""
        Write-Host "Tracked files have uncommitted changes:" -ForegroundColor Red
        foreach ($line in $dirty) {
            Write-Host ("  " + $line) -ForegroundColor Red
        }
        Write-Host ""
        Write-Host "This script clones HEAD, so those changes would not be tested and a green" -ForegroundColor Yellow
        Write-Host "result would mean nothing. Commit or stash them first, or pass -AllowDirty" -ForegroundColor Yellow
        Write-Host "to test the committed state only." -ForegroundColor Yellow
        exit 1
    }
    Write-Host ""
    Write-Host "-AllowDirty: the changes below are NOT part of this run. The clone comes from" -ForegroundColor Yellow
    Write-Host "HEAD, so only the committed state is tested." -ForegroundColor Yellow
    foreach ($line in $dirty) {
        Write-Host ("  " + $line) -ForegroundColor Yellow
    }
}

# ---------------------------------------------------------------------------------------
# Tools, up front. A missing cargo subcommand fails five minutes in with a message about an
# unknown command; finding it here costs a second and prints the install line.
# ---------------------------------------------------------------------------------------

$missing = New-Object System.Collections.ArrayList

if (-not (Test-ToolPresence -Executable "git" -ProbeArguments @())) {
    [void]$missing.Add("git is not on PATH. Install Git for Windows: https://git-scm.com/download/win")
}
if (-not (Test-ToolPresence -Executable "cargo" -ProbeArguments @())) {
    [void]$missing.Add("cargo is not on PATH. Install Rust: https://rustup.rs")
}
if (-not (Test-ToolPresence -Executable "cmake" -ProbeArguments @())) {
    [void]$missing.Add("cmake is not on PATH. Install CMake 3.20 or newer, then open a new shell.")
}
if (-not $SkipTauriBuild) {
    if (-not (Test-ToolPresence -Executable "cargo" -ProbeArguments @("tauri", "--version"))) {
        [void]$missing.Add("cargo tauri is missing. Install it with: cargo install tauri-cli --locked")
    }
}
if (-not (Test-ToolPresence -Executable "cargo" -ProbeArguments @("deny", "--version"))) {
    [void]$missing.Add("cargo deny is missing. Install it with: cargo install cargo-deny --locked")
}

if ($missing.Count -gt 0) {
    Write-Host ""
    Write-Host "Missing tools:" -ForegroundColor Red
    foreach ($line in $missing) {
        Write-Host ("  " + $line) -ForegroundColor Red
    }
    exit 1
}

# ---------------------------------------------------------------------------------------
# The warm half: one build directory, reused across runs and shared by every step.
# ---------------------------------------------------------------------------------------

$cacheRoot = Join-Path $env:LOCALAPPDATA "dile\ci-target"
if ($Clean) {
    Write-Note ("-Clean: removing the build cache at " + $cacheRoot)
    if (-not (Remove-Directory -Path $cacheRoot)) {
        Write-Host ("Could not remove " + $cacheRoot + ". Close anything holding a file there and retry.") -ForegroundColor Red
        exit 1
    }
}
New-Item -ItemType Directory -Force -Path $cacheRoot | Out-Null

# ---------------------------------------------------------------------------------------
# The fresh half: a shallow clone of HEAD in a directory of its own.
# ---------------------------------------------------------------------------------------

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$cloneDirectory = Join-Path $env:TEMP ("dile-ci-" + $stamp)
# file:// rather than a plain path: a plain local path makes git hardlink into the source
# object store, and --depth cannot be honoured. The URL form uses the normal transport.
$sourceUrl = "file:///" + ($repoRoot -replace "\\", "/")

Write-Banner "Cloning HEAD"
Write-Note ("    " + $sourceUrl + " -> " + $cloneDirectory)
& git clone --depth 1 --quiet $sourceUrl $cloneDirectory
if ($LASTEXITCODE -ne 0) {
    Write-Host "git clone failed." -ForegroundColor Red
    exit 1
}

$headSha = & git -C $cloneDirectory rev-parse --short HEAD
$headSubject = & git -C $cloneDirectory log -1 --pretty=%s
Write-Note ("    HEAD " + $headSha + "  " + $headSubject)

# Same environment as the workflow.
$env:CARGO_TERM_COLOR = "always"
$env:RUST_BACKTRACE = "1"
$env:CARGO_TARGET_DIR = $cacheRoot
Write-Note ("    CARGO_TARGET_DIR " + $cacheRoot)

# ---------------------------------------------------------------------------------------
# The steps, in the order ci.yml runs them.
# ---------------------------------------------------------------------------------------

$steps = New-Object System.Collections.ArrayList
[void]$steps.Add(@{ Name = "cmake --version"; Executable = "cmake"; Arguments = @("--version"); Optional = $false })
# Both sidecars, before anything compiles dile-app: `tauri-build` checks `bundle.externalBin`
# in the build script, so a missing one fails clippy and cargo test as well as the bundler.
# CPU-only and debug, exactly as the workflow runs it -- this is about the configuration
# being exercised, not about a binary that works.
[void]$steps.Add(@{ Name = "build-host.ps1 -Cpu"; Executable = "powershell"; Arguments = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts\build-host.ps1", "-Cpu", "-DebugBuild"); Optional = $false })
[void]$steps.Add(@{ Name = "cargo fmt --check"; Executable = "cargo"; Arguments = @("fmt", "--all", "--", "--check"); Optional = $false })
[void]$steps.Add(@{ Name = "cargo clippy"; Executable = "cargo"; Arguments = @("clippy", "--workspace", "--all-targets", "--", "-D", "warnings"); Optional = $false })
[void]$steps.Add(@{ Name = "cargo test"; Executable = "cargo"; Arguments = @("test", "--workspace"); Optional = $false })
[void]$steps.Add(@{ Name = "cargo tauri build"; Executable = "cargo"; Arguments = @("tauri", "build", "--debug", "--no-bundle"); Optional = $SkipTauriBuild })
[void]$steps.Add(@{ Name = "cargo deny check"; Executable = "cargo"; Arguments = @("deny", "check"); Optional = $false })

$results = New-Object System.Collections.ArrayList
$failed = $false
$totalWatch = [System.Diagnostics.Stopwatch]::StartNew()

Push-Location $cloneDirectory
try {
    foreach ($step in $steps) {
        if ($failed) {
            [void]$results.Add([pscustomobject]@{ Step = $step.Name; Status = "skipped"; Seconds = 0; ExitCode = 0 })
            continue
        }
        if ($step.Optional) {
            [void]$results.Add([pscustomobject]@{ Step = $step.Name; Status = "skipped"; Seconds = 0; ExitCode = 0 })
            continue
        }
        $result = Invoke-CiStep -Name $step.Name -Executable $step.Executable -StepArguments $step.Arguments
        [void]$results.Add($result)
        if ($result.Status -eq "FAILED") {
            $failed = $true
        }
    }
} finally {
    Pop-Location
}

$totalWatch.Stop()

# ---------------------------------------------------------------------------------------
# Verdict.
# ---------------------------------------------------------------------------------------

Write-Host ""
Write-Host "Summary" -ForegroundColor White
Write-Summary -Rows $results.ToArray()
Write-Host ""
Write-Note ("total " + (Format-Seconds -Value $totalWatch.Elapsed.TotalSeconds) + "s   HEAD " + $headSha)

if ($failed) {
    $red = $results | Where-Object { $_.Status -eq "FAILED" } | Select-Object -First 1
    Write-Host ""
    Write-Host ("FAILED: " + $red.Step + " exited with " + $red.ExitCode) -ForegroundColor Red
    Write-Host ("The clone was kept so the failure can be reproduced by hand:") -ForegroundColor Yellow
    Write-Host ("  " + $cloneDirectory) -ForegroundColor Yellow
    if ($SkipTauriBuild) {
        Write-Note 'Note: -SkipTauriBuild was on, so cargo tauri build did not run.'
    }
    exit 1
}

if (-not (Remove-Directory -Path $cloneDirectory)) {
    Write-Host ("Everything passed, but the clone could not be removed: " + $cloneDirectory) -ForegroundColor Yellow
}

Write-Host ""
if ($SkipTauriBuild) {
    Write-Host "Green, without the Tauri build. Run without -SkipTauriBuild before a release." -ForegroundColor Green
} else {
    Write-Host "Green. This is what the CI gate would say." -ForegroundColor Green
}
exit 0
