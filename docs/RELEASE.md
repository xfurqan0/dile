# Release checklist

Every command on this page is **for the maintainer to run**. Nothing here is run by CI or by
any assistant: publishing, tagging, submitting a package and changing a repository's
visibility are one-way doors, and they belong to a person.

Read it top to bottom the first time. Steps 0 to 5 are reversible; step 6 onwards is not.

---

## 0. Preconditions

- `docs/PROJECT.md` §6 says the work packages that get to a first release are done.
- `main` is clean and CI is green on it: lint, tests and the debug Tauri build on Windows,
  `cargo deny` on Ubuntu.
- You are signed in to GitHub as `xfurqan0`, with two-factor authentication on.

```powershell
git switch main
git pull --ff-only
git status --porcelain          # must print nothing

gh auth status                  # must show xfurqan0
gh run list --branch main --limit 3
```

---

## 1. Set the version

**For 0.1.0 this step is a read-back rather than an edit**: nothing has been released, so the
five files already say `0.1.0`. Check them anyway — the workflow refuses a tag that disagrees
with them, and finding that out from a red run is worse than finding it out here.

Five files carry it — two manifests and the three winget files.

```powershell
# Edit by hand, all in the same commit:
#   Cargo.toml                       -> [workspace.package] version = "0.1.0"
#   crates/dile-app/tauri.conf.json  -> "version": "0.1.0"
#   packaging/winget/*.yaml          -> PackageVersion: 0.1.0  (three files)

Select-String -Path Cargo.toml, crates\dile-app\tauri.conf.json -Pattern 'version'
Select-String -Path packaging\winget\*.yaml -Pattern 'PackageVersion'
```

`scripts\winget-manifest.ps1` writes the three winget files for you, and step 6 runs it again
with the real hash. The release workflow refuses a tag whose version does not match both
manifests, so a half-done bump fails the gate rather than the release.

Move the `## [Unreleased]` heading in `CHANGELOG.md` to `## [X.Y.Z] — YYYY-MM-DD` with today's
date, and read the README's status line and the CHANGELOG's opening paragraph back: both have
to describe a release that exists rather than one that is coming.

The `Cargo.lock` moves with the version. Let cargo do it rather than editing it:

```powershell
cargo check --workspace --locked   # fails if the lock file is behind
git add -A
git commit -m "Release 0.1.0"
```

---

## 2. Build from clean

A stale sidecar is the classic way to ship something that does not match the source. Start
from nothing — and on this project "nothing" includes the native library, because cargo does
not rebuild it when the flags that keep your account name out of it change.

```powershell
Remove-Item -Recurse -Force target, crates\dile-app\binaries -ErrorAction SilentlyContinue

powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

That one script does the four things whose order matters — the remapped environment, the two
sidecars with the GPU feature, `cargo tauri build`, then the check for build-machine paths —
and prints the artefacts with their sizes and the installer's SHA-256 at the end. Keep that
output; step 6 uses it.

Before it prints any of that it reads the three binaries back
(`scripts\check-binary-paths.ps1`) and, if any of them carries the path of the machine it was
built on, **deletes the bundle** rather than leave an installer on disk looking finished. So
an installer that exists at this point is one that passed. Step 5 runs the same check by hand.

Expected, on `x86_64-pc-windows-msvc`:

| Artefact | 0.1.0 |
|---|---|
| `dile-app.exe` | 5.83 MB |
| `dile-engine-host.exe` | 54.89 MB |
| `dile.exe` | 2.38 MB |
| `Dile_<version>_x64-setup.exe` | 9.30 MB |

Measured 2026-09-14 with the release profile in `Cargo.toml` (`opt-level = "s"`, `lto`, one
codegen unit, stripped, `panic = "abort"`) and the path remapping the script passes. The
engine host is most of it and always will be: it is the only binary that links
`transcribe.cpp`, with the Vulkan backend and ~1,977 compiled SPIR-V shaders in it. LZMA takes
66 MB of payload down to 9.3 MB, which is why the installer is a seventh of its contents.

An installer that is suddenly 30 MB means something got into the bundle. Look at
`bundle.resources` and `bundle.externalBin` in `crates/dile-app/tauri.conf.json` first.

---

## 3. Inspect the bundle

The installer must carry **five** things and nothing else:

```
dile-app.exe
dile-engine-host.exe
dile.exe
LICENSE.txt
THIRD-PARTY-NOTICES.md
uninstall.exe            (written by NSIS at install time, not packed)
```

Plus the panel and the settings window, which are compiled into `dile-app.exe` rather than
shipped as files. **No model**: the weights are downloaded on first run, with consent, and
`docs/PROJECT.md` §3 says why they are never put in a release. No fixtures, no recordings, no
documentation, no `.pdb`.

Read the file list out of the package itself:

```powershell
# 7-Zip reads an NSIS installer's contents without running it.
& "C:\Program Files\7-Zip\7z.exe" l target\release\bundle\nsis\Dile_0.1.0_x64-setup.exe
```

Then install it and look at what landed:

```powershell
Get-ChildItem -Recurse "$env:LOCALAPPDATA\Dile" | Select-Object Name, Length
```

---

## 4. Smoke the installer as a user would

```powershell
$setup = "target\release\bundle\nsis\Dile_0.1.0_x64-setup.exe"

# Silent, per user, no elevation prompt. /R is what starts it afterwards.
Start-Process $setup -ArgumentList "/S /R" -Wait

# It is running, it is on the Start Menu, and the command line answers.
Get-Process dile-app
Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Dile.lnk"
& "$env:LOCALAPPDATA\Dile\dile.exe" --version
& "$env:LOCALAPPDATA\Dile\dile.exe" transcribe <a wav file> --json
```

Then the thing no script can do. Put the caret in a Notepad, hold **`Ctrl+Alt+Space`**, say a
sentence, let go: the card appears, the countdown runs, the sentence lands in the window and
the clipboard comes back. Do it again in **Windows Terminal**, which needs the other chord.
Open the settings window from the tray, add a dictionary term, and check the next dictation
uses it.

**On a machine that has never run Dile, the first run is the thing being tested**: the consent
dialog, the download with its progress, the GPU probe, and the tray tooltip that says which
tier it landed on. That is a fresh Windows VM, and `docs/PROJECT.md` §6 WP7's acceptance
criterion is *install → first Turkish dictation in five minutes*.

Then the way out:

```powershell
Start-Process "$env:LOCALAPPDATA\Dile\uninstall.exe" -ArgumentList "/S" -Wait

# Nothing left behind that should not be.
Test-Path "$env:LOCALAPPDATA\Dile"
Get-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name Dile -ErrorAction SilentlyContinue
Test-Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Dile"
Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Dile.lnk"
```

All four must be absent or `False`.

**One thing is kept on purpose.** A silent uninstall — which is what `winget uninstall`
performs — never shows the *delete application data* page, so `%APPDATA%\io.github.xfurqan0.dile`
(settings, the dictionary, the tier record) and `%LOCALAPPDATA%\io.github.xfurqan0.dile`
(the models, which is a gigabyte) both stay. That is the right default for a reinstall and the
wrong one for somebody who meant it, so say so in the release notes. By hand:

```powershell
Remove-Item -Recurse "$env:APPDATA\io.github.xfurqan0.dile", "$env:LOCALAPPDATA\io.github.xfurqan0.dile"
```

---

## 5. Last read-through

```powershell
# No AI attribution anywhere in the history or the tree.
git log --all --format=%B | Select-String -Pattern "generated with|co-authored-by|claude\.ai/" -CaseSensitive:$false
git grep -niE "generated with|co-authored-by" -- . ':!Cargo.lock'

# No personal data, no absolute paths, no leftover secrets.
git grep -niE "yldz|@gmail|C:\\\\Users|/Users/|/home/" -- . ':!Cargo.lock'
git grep -nE "(sk-ant-|ghp_|xox[abprs]-|AIza)" -- . ':!Cargo.lock'

# The gates that answer these questions properly, on their own.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ci-local.ps1
```

All of it must come back empty or green. The only expected hits are the maintainer's own name
in `LICENSE`, `tauri.conf.json` and the winget manifests, and the placeholder paths in
`scripts\check-binary-paths.ps1`, `scripts\build-installer.ps1`, the two workflows and the
prose describing them. Read those: `C:\Users\<account>` and `C:\Users\runneradmin` are the
shapes being looked for, and neither names anybody.

**And inside the binaries, which none of the greps above can see.** Panic locations and
`GGML_ASSERT` compile their source path in as a string literal, so `strip = true` does not
remove them and a release build can carry the path every crate was compiled from — the account
name of whoever built it, shipped to everyone who downloads the installer.
`scripts\build-installer.ps1` passes `--remap-path-prefix` to rustc and `/d1trimfile:` to MSVC
for exactly this reason; here is the check that it worked:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-binary-paths.ps1
```

Expected, and the only acceptable answer:

```
target\release\dile-app.exe                          5.83 MB  0 match(es)
target\release\dile-engine-host.exe                 54.89 MB  0 match(es)
target\release\dile.exe                              2.38 MB  0 match(es)
no machine-specific paths in any of them
```

Any non-zero count is a release stopper, and the script prints the first five hits in context
so the escaped prefix names itself. Two things to know about what it is reading. It reads the
binaries **in `target\`**, not the copies already installed under `%LOCALAPPDATA%\Dile` — those
came from whichever build put them there. And only `scripts\build-installer.ps1` passes the
flags, so **any release build made another way replaces these files with unremapped ones**. If
anything has touched `target\release` since step 2, rerun the build before the check means
anything. A finding that names `ggml` is the C++ half, and its own fix is
`cargo clean -p transcribe-cpp-sys --release`.

Also confirm by eye:

- `LICENSE` — MIT, the right year and name, and the same name in `tauri.conf.json`'s
  `copyright` and in `packaging/winget/xfurqan0.dile.locale.en-US.yaml`.
- `THIRD-PARTY-NOTICES.md` — current.
- The README's model licences and source URLs — `docs/PROJECT.md` §3 requires them, and they
  are the one thing in the README a user could be misled by.
- `.gitattributes` — present, so line endings do not turn a first contributor's pull request
  into a whole-repository diff.

Write the release notes, which the workflow will attach to the draft:

```powershell
# docs/release-notes-0.1.0.md -- the CHANGELOG entry, shortened, with the known limits kept
# rather than buried. 0.1.0's is in the tree as the shape.
git add -A
git commit -m "Release notes for 0.1.0"
git push origin main
```

---

## 6. Tag

**From here on, nothing is reversible.**

```powershell
git tag -a v0.1.0 -m "Dile 0.1.0"
git push origin v0.1.0
```

The tag starts `.github/workflows/release.yml`: it re-runs the whole gate, checks the tag
against both manifests before it spends ten minutes on shaders, installs a pinned Vulkan SDK
on the runner, builds the installer, checks the binaries for build-machine paths before
anything is hashed or attached, writes `SHA256SUMS`, attests the build provenance, and creates
a **draft** release with both files attached. It does not publish.

```powershell
gh run watch
gh release view v0.1.0        # a draft, with two assets
```

Take the installer's hash from the release's own `SHA256SUMS` — the one built by the runner,
not the one on your laptop — and put it in the winget manifest:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\winget-manifest.ps1
git diff packaging/winget
winget validate --manifest packaging\winget
git add packaging/winget && git commit -m "winget: hash for 0.1.0" && git push
```

---

## 7. Publish the release

Read the draft on github.com first — the notes, the two assets, the file names. Then:

```powershell
gh release edit v0.1.0 --draft=false
```

Check what a user gets:

```powershell
gh release view v0.1.0 --web
gh attestation verify .\Dile_0.1.0_x64-setup.exe --repo xfurqan0/dile
```

---

## 8. Submit to winget

The package is `xfurqan0.dile`; the manifests are in `packaging/winget`, already validated.
Install from them one last time, which checks the hash against the file the release actually
serves:

```powershell
winget install --manifest packaging\winget
```

Then submit. **`wingetcreate` opens a pull request against `microsoft/winget-pkgs`, which is a
message that leaves this machine**: read the text it produces before it goes, and let it carry
no AI attribution — the same rule as every commit here.

```powershell
winget install --id Microsoft.WingetCreate --exact
wingetcreate submit --token <GitHub PAT with public_repo> packaging\winget
```

Merging takes a day or two and is done by their automation plus a reviewer. When it lands:

```powershell
winget show xfurqan0.dile
winget install xfurqan0.dile
```

---

## 9. Apply to SignPath Foundation

**After** the release is out, not before — [docs/CODE_SIGNING.md](CODE_SIGNING.md) has the
reasoning and the conditions. The application asks for a released, actively maintained
project; give the repository a little history first, then apply at
[signpath.org/apply](https://signpath.org/apply) with:

- the repository URL, and `docs/CODE_SIGNING.md` as the published signing policy;
- `.github/workflows/release.yml` as the CI that builds from source;
- the release itself, with its attestation.

If it is approved: add the four repository secrets, replace the `sign` job's `if: false` with
`github.event_name == 'workflow_dispatch' && inputs.sign` — the condition is written out in a
comment directly above it, and the `sign` input it reads is already declared — and change the
README's "unsigned" paragraph. Three lines, in one reviewed commit. Replace rather than delete:
a `sign` job with no condition at all would sign on every tag push, which is the opposite of
the manual approval `docs/CODE_SIGNING.md` promises.

---

## 10. After the release

- Leave `main` on the released version until the next change lands, then bump it with a
  `## [Unreleased]` section in `CHANGELOG.md`. A `main` that is already on the registry's
  version with no note is how a mystery starts. **The section goes in as soon as something
  lands and the version numbers do not move with it:** step 1 sets them in the release commit,
  in the same change that renames the heading to `## [X.Y.Z] — YYYY-MM-DD`, so a tree is never
  carrying a version no build was ever made from.
- Watch the first issues. A product that runs a GPU probe on first start gets "it says CPU
  tier" reports; the first two things to ask for are the tray tooltip and
  `%APPDATA%\io.github.xfurqan0.dile\engine.json`, which holds exactly what the probe saw.
- **The README has no GIF yet.** It needs a screen recording of one dictation — hold, speak,
  the card, the paste — and a recording is a thing a person makes. Until then the README
  describes the loop in words. `docs/PROJECT.md` §7 keeps it on the open list.
- No updater endpoint and no minisign key exists for any release so far. Before turning one
  on: generate the key with `cargo tauri signer generate`, keep the private key in a password
  manager and a second offline copy, and never put it in the repository. Losing it means never
  shipping an update to the installed base again; `.gitignore` already refuses `*.key`.
