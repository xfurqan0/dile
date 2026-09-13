# Security

Dile listens to a microphone and pastes into whatever window has focus. That is a category
of tool that could leak everything you say and everything you type, so this page is specific
about what it opens, what it never opens, and what leaves the machine.

## Reporting a vulnerability

Use GitHub's private reporting: **Security → Report a vulnerability** on
[github.com/xfurqan0/dile](https://github.com/xfurqan0/dile/security/advisories/new). It
reaches the maintainer and nobody else. Please do not open a public issue for something that
discloses a way to make Dile record without showing that it is recording, to make it paste
somewhere it was not asked to, or to get audio or text off the machine.

Expect a first answer within a week. This is a one-person project with no service behind it,
so there is no on-call rotation to promise you — what there is, is a small attack surface and
a short list of files.

Only the latest release is supported. There is no back-porting; the fix is the next version.

## The promise

**Audio and text never leave the machine.** No telemetry, no analytics, no crash reporter,
no account. The only network call Dile makes is downloading a model from Hugging Face, after
an explicit consent dialog, and it is the only one there will ever be.

## What it will read, and when

WP0 is a tray icon and reads nothing at all. As the packages land, this is the whole list —
and it is written here first so that anything not on it is a bug.

| Source | What is taken | Package |
|---|---|---|
| The default microphone | Audio, into memory, only while the hotkey is held or a toggled recording is running | WP2 |
| The focused window's handle and title | The paste target and the label the panel shows, so the destination is never a surprise | WP5 |
| The clipboard | Its previous contents, held in memory so they can be restored after a paste | WP5 |
| `%APPDATA%\dile\` | Dile's own settings and dictionary | WP5 |
| The model cache directory | The GGUF weights Dile downloaded | WP3 |

**Audio is never written to disk.** It is processed in memory and dropped. There is no
history, no transcript log and no "recent dictations" list — features other tools have, and
deliberate exclusions here.

## What it never reads

- **Keystrokes.** The global hook exists to see one hotkey and the panel's Enter, Esc and
  Ctrl+C while the panel is visible. It is not a key logger and must never become one.
- **Window contents.** Dile learns the title and the process of the window it will paste
  into, so it can pick the right paste chord and show you the destination. It does not read
  what is in it.
- **Any credential file.** There are no accounts and no tokens in this product.

## The engine runs in its own process

`docs/PROJECT.md` §3 puts the speech engine in a separate process, and the reason is
robustness as much as safety: a GPU driver fault in the inference path takes that process
down and leaves the tray running. `dile-app` therefore does **not** depend on `dile-engine`
in the workspace, and it is not an oversight when you notice that.

The GPU backend is Vulkan and it is **the default tier**, but it is never assumed. On first
start Dile runs a probe transcription inside that isolated engine process; only a device that
comes back with a correct result is used, and a machine where the probe fails or crashes gets
the CPU tier with a small model instead. That is the difference from an "Auto" setting, and
the reason for it is Handy's open issue #1755, where a Vulkan auto-GPU path bug-checks an
RTX 5090: a driver-level fault is not something a process boundary can contain, so the
boundary is where the probe runs and the tier stays visible and switchable in settings.

## The webview

The panel is a WebView2 page with a strict content security policy (`tauri.conf.json`) and
one capability file (`crates/dile-app/capabilities/default.json`) that says what it may do
and why. It is granted `core:default` plus permission to hide its own window, and nothing
else: no shell, no dialogs, no filesystem, no HTTP. Every permission added there is a
permission the page can be made to use by anything that gets into it.

## Models

Weights are downloaded from Hugging Face, pinned by commit sha, with a sha256 the application
carries and checks. A download that does not match is discarded. Dile hosts no mirror and
puts no models on GitHub Releases.
