# Packaging

## winget manifests

The three files in `winget/` are the package as
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) wants it: a version
manifest that routes, an installer manifest, and a default-locale manifest that carries
everything `winget show` prints.

They live here rather than only in the pull request so that the values that have to be right
— the identifier, the switches, the scope, the licence, the tags — are reviewed in the same
repository as the thing they describe, and so that the next version is a diff rather than a
rewrite.

## Why winget at all

An unsigned installer downloaded in a browser gets a SmartScreen warning, and the warning is
the one a first-time user reads as "this is malware". `winget install` does not go through
the browser and does not raise it. Until SignPath Foundation approves the certificate
([docs/CODE_SIGNING.md](../docs/CODE_SIGNING.md)), winget is the install path this project
points people at, and `README.md` says so in those words.

winget accepts unsigned installers. It requires the download to be reachable over HTTPS and
to match `InstallerSha256` exactly, which is a weaker promise than a signature — it says the
bytes are the ones the manifest was written for, not who built them — but it is the promise
that is available today, and it is checked on every install.

## Before submitting

`InstallerSha256` in `xfurqan0.dile.installer.yaml` is sixty-four zeros. It cannot be
anything else until the release exists: it is the hash of a file that is uploaded in the step
before. [docs/RELEASE.md](../docs/RELEASE.md) is the checklist, and step 6 is where the real
hash goes in — with `scripts/winget-manifest.ps1`, which takes it from the release's own
`SHA256SUMS` rather than from a file on a laptop.

Validate locally first — this needs no network and no account:

```powershell
winget validate --manifest packaging\winget
```

Then, with the release published and the hash filled in:

```powershell
# Installs from the manifest as a user would, and checks the hash against the download.
winget install --manifest packaging\winget
```

## Submitting

The pull request goes to `microsoft/winget-pkgs`, under
`manifests/x/xfurqan0/dile/0.1.0/`. `wingetcreate` does the fork, the branch and the pull
request in one command:

```powershell
winget install --id Microsoft.WingetCreate --exact
wingetcreate submit --token <a GitHub PAT with public_repo> packaging\winget
```

**The pull request text is a message that leaves this machine.** It is written by the
maintainer, read before it is sent, and carries no AI attribution — same rule as every commit
in this repository.
