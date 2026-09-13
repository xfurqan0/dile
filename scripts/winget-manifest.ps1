#Requires -Version 5.1
<#
.SYNOPSIS
    Fills the winget manifests in from a release that exists.

.DESCRIPTION
    Three files in `packaging\winget\` carry a version, and one of them carries the SHA-256 of
    an installer that does not exist until the release does. This is the edit that connects
    them, so that `docs/RELEASE.md` step 6 is one command rather than four careful pastes.

    **The hash comes from the release, not from this machine.** By default it is read out of
    the `SHA256SUMS` the release workflow attached to the draft — the hash of the installer a
    GitHub runner built from the tagged commit. An installer built on a laptop has a different
    hash and no provenance, and a manifest carrying it would tell everybody who installs the
    package to expect a file nobody else can reproduce.

    What it writes:

        xfurqan0.dile.yaml               PackageVersion
        xfurqan0.dile.locale.en-US.yaml  PackageVersion, ReleaseNotesUrl
        xfurqan0.dile.installer.yaml     PackageVersion, InstallerUrl, InstallerSha256

    It rewrites those lines and nothing else: the switches, the scope, the tags and every
    comment stay exactly as they were reviewed.

.PARAMETER Version
    The version to write, without the `v`. Read from `Cargo.toml` when it is not given.

.PARAMETER FromFile
    Take the hash from a local installer instead of from the release. For a rehearsal, and it
    says so on the way out — **never** for a manifest that gets submitted.

.PARAMETER Repository
    The GitHub repository the release is on. `xfurqan0/dile` unless something has moved.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\winget-manifest.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\winget-manifest.ps1 -FromFile target\release\bundle\nsis\Dile_0.1.0_x64-setup.exe
#>

[CmdletBinding()]
param(
    [string]$Version,
    [string]$FromFile,
    [string]$Repository = "xfurqan0/dile"
)

$ErrorActionPreference = "Continue"

# Verb-Noun names only: a function named after a built-in alias silently loses to the alias.
function Write-Note {
    param([string]$Text)
    Write-Host $Text -ForegroundColor DarkGray
}

function Set-ManifestLine {
    # One key, one value, the rest of the file untouched. A YAML library would be a dependency
    # for a job that is four lines of text, and it would reformat every comment on its way
    # past them.
    param(
        [string]$Path,
        [string]$Key,
        [string]$Value
    )
    $text = [System.IO.File]::ReadAllText($Path)
    $pattern = "(?m)^" + [regex]::Escape($Key) + ":[ \t]*\S.*$"
    $replacement = $Key + ": " + $Value
    if (-not [regex]::IsMatch($text, $pattern)) {
        Write-Host ("no top-level " + $Key + " in " + (Split-Path $Path -Leaf)) -ForegroundColor Red
        return $false
    }
    $text = [regex]::Replace($text, $pattern, $replacement)
    [System.IO.File]::WriteAllText($Path, $text, (New-Object System.Text.UTF8Encoding($false)))
    return $true
}

function Set-InstallerValue {
    # `InstallerUrl` and `InstallerSha256` are nested under `Installers:`, so they are indented
    # and the anchored pattern above will not see them.
    param(
        [string]$Path,
        [string]$Key,
        [string]$Value
    )
    $text = [System.IO.File]::ReadAllText($Path)
    $pattern = "(?m)^(\s+)" + [regex]::Escape($Key) + ":[ \t]*\S.*$"
    if (-not [regex]::IsMatch($text, $pattern)) {
        Write-Host ("no " + $Key + " under Installers in " + (Split-Path $Path -Leaf)) -ForegroundColor Red
        return $false
    }
    $text = [regex]::Replace($text, $pattern, ('${1}' + $Key + ": " + $Value))
    [System.IO.File]::WriteAllText($Path, $text, (New-Object System.Text.UTF8Encoding($false)))
    return $true
}

$scriptDirectory = $PSScriptRoot
if ([string]::IsNullOrEmpty($scriptDirectory)) {
    $scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
}
$repoRoot = (Resolve-Path (Split-Path -Parent $scriptDirectory)).Path.TrimEnd('\')
$winget = Join-Path $repoRoot "packaging\winget"

if ([string]::IsNullOrEmpty($Version)) {
    $line = Select-String -Path (Join-Path $repoRoot "Cargo.toml") -Pattern '^version = "(.+)"' |
        Select-Object -First 1
    if ($null -eq $line) {
        Write-Host "Cargo.toml has no workspace version to read." -ForegroundColor Red
        exit 1
    }
    $Version = $line.Matches[0].Groups[1].Value
}

$installerName = "Dile_" + $Version + "_x64-setup.exe"
$installerUrl = "https://github.com/" + $Repository + "/releases/download/v" + $Version + "/" + $installerName

Write-Host ""
Write-Host "winget manifests" -ForegroundColor White
Write-Note ("version     " + $Version)
Write-Note ("installer   " + $installerName)

# ---------------------------------------------------------------------------------------
# The hash, from the release unless a rehearsal says otherwise.
# ---------------------------------------------------------------------------------------

$hash = $null
if (-not [string]::IsNullOrEmpty($FromFile)) {
    if (-not (Test-Path -LiteralPath $FromFile)) {
        Write-Host ($FromFile + " is not there.") -ForegroundColor Red
        exit 1
    }
    $hash = (Get-FileHash -LiteralPath $FromFile -Algorithm SHA256).Hash.ToLower()
    Write-Note ("hash        " + $hash + "  (from a local file)")
} else {
    if ($null -eq (Get-Command gh -ErrorAction SilentlyContinue)) {
        Write-Host "gh is not on PATH, so the release's SHA256SUMS cannot be read." -ForegroundColor Red
        Write-Host "Install the GitHub CLI, or pass -FromFile for a rehearsal." -ForegroundColor Yellow
        exit 1
    }
    $temporary = Join-Path $env:TEMP ("dile-winget-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $temporary | Out-Null
    & gh release download ("v" + $Version) --repo $Repository --pattern SHA256SUMS --dir $temporary
    if ($LASTEXITCODE -ne 0) {
        Write-Host ("v" + $Version + " has no SHA256SUMS to download yet.") -ForegroundColor Red
        Write-Host "docs/RELEASE.md step 6: the tag comes first, and the workflow attaches it." -ForegroundColor Yellow
        Remove-Item -Recurse -Force $temporary -ErrorAction SilentlyContinue
        exit 1
    }
    foreach ($row in Get-Content (Join-Path $temporary "SHA256SUMS")) {
        $parts = $row -split '\s+' | Where-Object { $_ }
        if ($parts.Count -ge 2 -and $parts[1] -eq $installerName) {
            $hash = $parts[0].ToLower()
        }
    }
    Remove-Item -Recurse -Force $temporary -ErrorAction SilentlyContinue
    if ($null -eq $hash) {
        Write-Host ("SHA256SUMS does not list " + $installerName) -ForegroundColor Red
        exit 1
    }
    Write-Note ("hash        " + $hash + "  (from the release)")
}

if ($hash -notmatch '^[0-9a-f]{64}$') {
    Write-Host ("that is not a sha256: " + $hash) -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------------------------------------
# The edits.
# ---------------------------------------------------------------------------------------

$ok = $true
$ok = (Set-ManifestLine -Path (Join-Path $winget "xfurqan0.dile.yaml") -Key "PackageVersion" -Value $Version) -and $ok
$ok = (Set-ManifestLine -Path (Join-Path $winget "xfurqan0.dile.locale.en-US.yaml") -Key "PackageVersion" -Value $Version) -and $ok
$ok = (Set-ManifestLine -Path (Join-Path $winget "xfurqan0.dile.locale.en-US.yaml") -Key "ReleaseNotesUrl" -Value ("https://github.com/" + $Repository + "/releases/tag/v" + $Version)) -and $ok
$ok = (Set-ManifestLine -Path (Join-Path $winget "xfurqan0.dile.installer.yaml") -Key "PackageVersion" -Value $Version) -and $ok
$ok = (Set-InstallerValue -Path (Join-Path $winget "xfurqan0.dile.installer.yaml") -Key "InstallerUrl" -Value $installerUrl) -and $ok
$ok = (Set-InstallerValue -Path (Join-Path $winget "xfurqan0.dile.installer.yaml") -Key "InstallerSha256" -Value $hash) -and $ok

if (-not $ok) {
    Write-Host ""
    Write-Host "Some values were not found. The manifests may be half-written; check git diff." -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "Written. Read the diff, then:" -ForegroundColor Green
Write-Host "  winget validate --manifest packaging\winget"
if (-not [string]::IsNullOrEmpty($FromFile)) {
    Write-Host ""
    Write-Host "-FromFile: that hash is a local build's, with no provenance behind it." -ForegroundColor Yellow
    Write-Host "Do not submit this. docs/RELEASE.md step 6 takes the hash from the release." -ForegroundColor Yellow
}
exit 0
