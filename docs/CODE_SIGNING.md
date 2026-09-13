# Code signing

**Dile 0.1.0 is not signed.** This page says what that means for you, what is being done
about it, and what the signing pipeline will look like when it exists — written now, while
the decisions are fresh, rather than after the certificate arrives.

## What unsigned means today

Windows shows the installer to Microsoft's reputation service, SmartScreen, and an executable
with no publisher and no download history is exactly what SmartScreen is built to interrupt.
Two paths, two experiences:

| How you install | What you see |
|---|---|
| `winget install xfurqan0.dile` | Nothing. winget does not go through the browser, so the download warning never appears. It checks the SHA-256 in the manifest against the file it fetched. |
| Downloading the `.exe` from GitHub Releases | "Windows protected your PC". **More info → Run anyway.** Some browsers also warn while downloading, because the file is uncommon rather than because anything is known about it. |

This is why the README points at winget first. It is not a workaround: a hash checked against
a manifest in a public repository is a real, if different, guarantee — it says the bytes are
the ones the package was reviewed with.

**There is a second reason this project wants a signature, and it is specific to what Dile
does.** A push-to-talk dictation tool installs a low-level keyboard hook and opens a
microphone. Those two things together are, byte for byte, what a keylogger with audio capture
looks like to an endpoint-protection product, and an unsigned binary doing them has nothing to
weigh against the heuristic. What Dile actually does with them is in
[SECURITY.md](../SECURITY.md) — no disk, no socket, no telemetry — and the source is the whole
of the answer today. A signature would not change the behaviour; it would give the scanner
something to attribute it to.

## What you can check yourself

Every release carries a `SHA256SUMS` file listing the installer's hash, and GitHub's
[build attestation](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations)
for the same artefact. The attestation is the stronger of the two: it is signed by GitHub and
says which workflow run, from which commit, in which repository, produced that exact file.

```powershell
# The hash, against the release's own list.
Get-FileHash .\Dile_0.1.0_x64-setup.exe -Algorithm SHA256

# The provenance, against GitHub's record. Needs the GitHub CLI.
gh attestation verify .\Dile_0.1.0_x64-setup.exe --repo xfurqan0/dile
```

## The plan: SignPath Foundation

[SignPath Foundation](https://signpath.org/) gives free code-signing certificates to open
source projects, with the signing itself performed on their infrastructure, driven by CI, and
released by a human approval per release. The publisher shown to Windows is "SignPath
Foundation" rather than the maintainer, which is the point: the identity being vouched for is
the project's, verified by them.

### Why the application comes after the first release, not before

SignPath Foundation asks that a project **already be released** and **actively maintained**.
A project applying with no release and no history is asking to be evaluated on a promise. So
the order is: release 0.1.0 unsigned → let the repository accumulate a little history → apply
→ if approved, sign from the next release onwards. The first release does not wait for a
certificate, and this page is what ships in its place.

*(The same decision nazar-tray took, for the same reason. The alternative — hold the release
until the certificate arrives — was rejected because the queue is other people's and the
deadline would be theirs.)*

### The conditions, and where this project stands

| Condition | Status |
|---|---|
| OSI-approved licence | MIT, `LICENSE`. |
| No proprietary components | Every dependency is permissive; `deny.toml` fails the build otherwise, and `THIRD-PARTY-NOTICES.md` is the list. The speech weights are not a dependency — they are downloaded at run time, with consent, and the README lists their licence and source. |
| Built from source in CI | `.github/workflows/release.yml` builds the installer on a GitHub-hosted runner from a tagged commit, including the Vulkan SDK it needs. Nothing is built on a laptop and uploaded. |
| Published signing policy | This page. |
| Two-factor authentication on the source repository account | On for `xfurqan0`. |
| Named author / reviewer / approver roles | One person holds all three today, which SignPath permits for a single-maintainer project and which this page states rather than hides. If the project gains a second regular contributor, author and approver are split — the approver is the one who must not be the one who pushed the tag. |
| Manual approval per release | The signing job is switched off today, and the condition that turns it on — written in a comment directly above it — is `github.event_name == 'workflow_dispatch' && inputs.sign`: a dispatch somebody chose, with the `sign` input ticked, never a tag push. The `sign` input is already declared in the workflow. A release that nobody approved is a release that does not get signed. |
| "SignPath Foundation" visible as publisher | Accepted. The README will say who signs and why the name is not the maintainer's. |
| Already released, actively maintained | The reason this is step two. |

### The CI job, written and switched off

`.github/workflows/release.yml` carries the signing job with `if: false` on it. It is there so
that the shape of the pipeline is reviewable now — the artefact it would sign, where the
signature would go, which secrets it would need — and so that turning it on is a one-line
change reviewed on its own rather than a new file written under time pressure.

That one line is a replacement, not a deletion: `if: false` becomes

```yaml
    if: ${{ github.event_name == 'workflow_dispatch' && inputs.sign }}
```

which is the condition written out in a comment immediately above it, so the job that is off
already says what "on" means. Deleting `if: false` outright would sign on every tag push,
which is the one thing this table's *manual approval per release* row promises it does not do.

What it will need when it is turned on:

| Secret | What it is |
|---|---|
| `SIGNPATH_API_TOKEN` | The CI user's token. Repository secret, never in the workflow file. |
| `SIGNPATH_ORGANIZATION_ID` | Given at approval. |
| `SIGNPATH_PROJECT_SLUG` | `dile`. |
| `SIGNPATH_SIGNING_POLICY_SLUG` | `release-signing`, once the policy exists. |

The job uploads the unsigned installer as a workflow artefact, calls SignPath's action to
submit it, waits for the human approval, and downloads the signed file back. **Signing
happens after the tests and after the bundle, and before anything is attached to a release**
— so a release either has the signed installer or has no installer.

### What is signed

The installer, and all three programs inside it:

| Binary | Why it in particular |
|---|---|
| `dile-app.exe` | The one that installs the low-level keyboard hook and opens the microphone. The most likely of the three to be looked at by an endpoint-protection product, and the one a user sees named in a SmartScreen dialog. |
| `dile-engine-host.exe` | 55 MB of compiled inference runtime that loads a gigabyte of weights and talks to a GPU driver. Large, native, and started by another process — three things a heuristic notices. |
| `dile.exe` | The command line. Small, but it is the one somebody will put in a script and hand to a colleague, where an unsigned binary is a harder conversation. |

Leaving any of them unsigned inside a signed installer would be the worst of both.

## The alternatives, and why not

| Option | Why not |
|---|---|
| **Certum Open Source** (~€25–69/yr) | The certificate lives on a hardware token, so signing cannot happen in CI — every release would be signed on a laptop, which is the thing SignPath's model exists to avoid. Their terms also allow revocation if the software is distributed commercially, which is a condition this project would rather not carry. |
| **Azure Trusted Signing** | Individual developers are limited to the United States and Canada; the organisation route requires a verifiable legal entity in a list that does not include the maintainer's country. This is the one hard blocker. |
| **A commercial OV/EV certificate** (~$200–600/yr) | Above this project's whole budget ceiling, and since 2024 an EV certificate no longer buys an automatic SmartScreen pass anyway — reputation is built by downloads either way. |
| **Ship unsigned for ever** | What 0.1.0 does, deliberately, with the warning documented rather than hidden. It is a starting point, not an answer. |

## macOS and Linux

Not applicable yet: v1 is Windows only, and `docs/PROJECT.md` §6 puts the other two platforms
in v2. A macOS build needs an Apple Developer account ($99/yr) for notarisation, which is a
separate decision with its own budget line, and it is not taken. Linux ships no installer.
