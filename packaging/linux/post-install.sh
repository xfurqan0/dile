#!/bin/sh
# Make the keyboard rule this package just installed take effect, without a logout.
#
# One file for both packages, deliberately. `tauri-bundler` copies it into the .deb as
# `postinst` — where the shebang matters — and reads the same bytes into the .rpm's `%post`
# scriptlet, where rpm runs the body under /bin/sh and the shebang is a comment. Two files
# would be two chances for one of them to drift.
#
# **Nothing here may fail the installation.** The rule is on disk either way, and a udev that
# will not reload is a logout away from the same result; an installer that exits non-zero over
# it would leave a half-installed package behind for a problem that is not one. Hence the
# guard and the `|| true`: a container, a chroot, an image build or a machine with no udev
# running are all ordinary places for this to be a no-op.
#
# What the rule grants is in the rule itself and in docs/BUILDING.md, "Keyboard access on
# Linux". The short version: while you are logged in at this machine, a program running as you
# can read the keyboard whatever window has the focus. That is what a push-to-talk trigger on
# a lone right Ctrl costs under Wayland, and Dile's side of the bargain is that the audio and
# the text never leave the machine.

set -e

if ! command -v udevadm >/dev/null 2>&1; then
    exit 0
fi

udevadm control --reload-rules || true
# `--subsystem-match=input` rather than a bare trigger: the rule is about input devices, and
# re-triggering every device on the machine to apply one file is a great deal of work for a
# great many devices that have nothing to do with it.
udevadm trigger --subsystem-match=input || true

exit 0
