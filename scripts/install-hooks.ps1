#Requires -Version 5.1
<#
.SYNOPSIS
    Installs the git hooks this repository uses. Today that is one: pre-push.

.DESCRIPTION
    The pre-push hook runs scripts/ci-local.ps1 -SkipTauriBuild, which is the quick half of
    the CI gate: formatting, clippy, the test suite and cargo deny, against a fresh clone of
    HEAD. The Tauri build is left out on purpose, because it is the slow step and a push is
    not the last chance to catch it.

    Running this script again is a no-op when the hook is already the current one, so it is
    safe to run after every pull.

    A hook is a local convenience, not a wall. `git push --no-verify` skips it, and that is
    the documented escape hatch when the gate is red for a reason that has nothing to do with
    the change being pushed.

.PARAMETER Force
    Overwrites a pre-push hook this script did not write, after copying it to pre-push.bak.
    Without it, a foreign hook is left alone and the script exits non-zero.

.PARAMETER Uninstall
    Removes the hook, if it is the one this script installed.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-hooks.ps1
#>

[CmdletBinding()]
param(
    [switch]$Force,
    [switch]$Uninstall
)

$ErrorActionPreference = "Continue"

# The marker is how the script recognises its own work on a later run. Changing it orphans
# every hook already installed, so it does not change.
$marker = "# dile-ci-gate"

$hookLines = @(
    '#!/bin/sh',
    '# dile-ci-gate',
    '#',
    '# Installed by scripts/install-hooks.ps1. Runs the quick half of the local CI mirror',
    '# (formatting, clippy, tests, cargo deny) against a fresh clone of HEAD before the push',
    '# leaves the machine, because GitHub Actions cannot run this repository right now.',
    '#',
    '# Skip it with: git push --no-verify',
    '',
    'repo_root=$(git rev-parse --show-toplevel)',
    'exec powershell -NoProfile -ExecutionPolicy Bypass -File "$repo_root/scripts/ci-local.ps1" -SkipTauriBuild',
    ''
)
# LF, and no byte order mark: Git for Windows hands the file to sh, and sh chokes on both
# CRLF and a BOM.
$hookText = [string]::Join("`n", $hookLines)

function Write-ShellFile {
    param(
        [string]$Path,
        [string]$Text
    )
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Text, $encoding)
}

function Get-NormalisedText {
    param([string]$Text)
    $out = $Text -replace "`r`n", "`n"
    return $out
}

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

# rev-parse rather than a hard-coded .git\hooks: a worktree keeps its hooks somewhere else.
$hooksDirectory = & git -C $repoRoot rev-parse --git-path hooks
if ($LASTEXITCODE -ne 0) {
    Write-Host "Could not locate the hooks directory." -ForegroundColor Red
    exit 1
}
if (-not [System.IO.Path]::IsPathRooted($hooksDirectory)) {
    $hooksDirectory = Join-Path $repoRoot $hooksDirectory
}
$hooksDirectory = [System.IO.Path]::GetFullPath($hooksDirectory)
$hookPath = Join-Path $hooksDirectory "pre-push"

if (-not (Test-Path -LiteralPath $hooksDirectory)) {
    New-Item -ItemType Directory -Force -Path $hooksDirectory | Out-Null
}

if ($Uninstall) {
    if (-not (Test-Path -LiteralPath $hookPath)) {
        Write-Host "No pre-push hook installed. Nothing to do."
        exit 0
    }
    $existing = Get-NormalisedText -Text ([System.IO.File]::ReadAllText($hookPath))
    if ($existing.Contains($marker)) {
        Remove-Item -LiteralPath $hookPath -Force
        Write-Host ("Removed " + $hookPath) -ForegroundColor Green
        exit 0
    }
    Write-Host ("The pre-push hook at " + $hookPath + " was not written by this script. Left alone.") -ForegroundColor Yellow
    exit 1
}

if (Test-Path -LiteralPath $hookPath) {
    $existing = Get-NormalisedText -Text ([System.IO.File]::ReadAllText($hookPath))
    if ($existing -eq $hookText) {
        Write-Host ("pre-push is already current: " + $hookPath) -ForegroundColor Green
        Write-Host "Bypass it for a single push with: git push --no-verify" -ForegroundColor DarkGray
        exit 0
    }
    if (-not $existing.Contains($marker)) {
        if (-not $Force) {
            Write-Host ("A pre-push hook is already installed and this script did not write it:") -ForegroundColor Red
            Write-Host ("  " + $hookPath) -ForegroundColor Red
            Write-Host "Pass -Force to replace it. The old one is copied to pre-push.bak first." -ForegroundColor Yellow
            exit 1
        }
        $backupPath = $hookPath + ".bak"
        Copy-Item -LiteralPath $hookPath -Destination $backupPath -Force
        Write-Host ("Existing hook backed up to " + $backupPath) -ForegroundColor Yellow
    }
}

Write-ShellFile -Path $hookPath -Text $hookText
Write-Host ("Installed " + $hookPath) -ForegroundColor Green
Write-Host ""
Write-Host "Every push now runs: scripts\ci-local.ps1 -SkipTauriBuild"
Write-Host "  formatting, clippy, tests and cargo deny, against a fresh clone of HEAD."
Write-Host "  The Tauri build is not in it; run the script without the switch for that."
Write-Host ""
Write-Host "Bypass it for a single push with: git push --no-verify" -ForegroundColor DarkGray
exit 0
