# Packaging

## linux/70-dile-input.rules

The udev rule that lets the trigger see the keyboard. Dile reads `/dev/input/event*` rather
than asking a window system, which is the only way hold-to-talk on a lone right Ctrl works
under Wayland — and no distribution grants that to an ordinary user by default.

It is `TAG+="uaccess"` and not an `input` group membership, deliberately: logind gives the
device to whoever is logged in at the seat and takes it back when they log out, where
`usermod -aG input` would give every process that account ever starts — an SSH session
included — the ability to read every keystroke on the machine for as long as the account
exists. The file says the same thing at length, including what the rule honestly costs.

**The packages install it**, at `/usr/lib/udev/rules.d/`, and run `post-install.sh`
afterwards so that it takes effect without a logout. The alternative — asking at first launch
through `pkexec` — was declined: a tray application raising a password prompt about a file
only root can write, at the moment somebody first presses a key, is a worse question than the
same one asked by `dnf install`. A source build installs the file by hand;
`docs/BUILDING.md`, "Keyboard access on Linux", is the two commands and what they grant, and
`dile-hotkey` names the file in the one error a person who needs it will actually see.

## linux/post-install.sh

`udevadm control --reload-rules` and `udevadm trigger --subsystem-match=input`, guarded so
that neither can fail an installation. **One file for both packages**: `tauri-bundler` copies
it into the `.deb` as `postinst`, where the shebang matters, and reads the same bytes into the
`.rpm`'s `%post`, where rpm runs the body under `/bin/sh` and the shebang is a comment.

## What the Linux packages depend on, and what decides it

`tauri-cli` writes four of the dependencies itself — webkit2gtk, gtk3 and the appindicator, as
package names in the `.deb` and as sonames in the `.rpm` — and **it takes the appindicator's
name from what pkg-config can see on the building machine**. With
`ayatana-appindicator3-0.1` present the packages name the maintained library; without it they
quietly name the 2018 `libappindicator3-1` instead, and nothing says so.
`scripts/build-installer.sh` checks for it before it compiles anything and refuses to build a
release without it, because a published package that asks for the wrong library is worse than
one that does not exist.

Everything else is written by hand in `crates/dile-app/tauri.linux.conf.json`, and the list is
short on purpose — a dependency that is already implied by another one is a line that can only
go stale:

| Declared | Why it is not implied |
|---|---|
| ALSA (`libasound2t64 \| libasound2`, `libasound.so.2`) | `dile-capture` links it directly, and nothing else in the tree pulls it in. cpal reaches PipeWire through it, so it is needed on a PipeWire desktop too |
| `libstdc++6` / `libstdc++.so.6` | `dile-engine-host` is the one binary here with C++ in it, and it is a sidecar rather than the thing the bundler looks at |

Not declared, deliberately: **libX11 and libXi**, which `x11-dl` opens at run time and which
GTK3 already requires; and **ca-certificates**, because the model download goes through
`rustls` with `webpki-roots` compiled in and never reads the system trust store.

The `|` in the ALSA line is Debian's own alternative syntax, and `tauri-bundler` writes the
string through verbatim. `libasound2` was renamed `libasound2t64` in the 64-bit-time_t
transition, so a package built on Ubuntu 22.04 that named only the old one would be relying on
a compatibility `Provides` that exists today and is not promised forever.

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
