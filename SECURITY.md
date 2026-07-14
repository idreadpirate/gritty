# Security Policy

## Reporting a vulnerability

Please report vulnerabilities privately via
[GitHub Security Advisories](https://github.com/idreadpirate/gritty/security/advisories/new)
— do not open a public issue for anything exploitable. You'll get a response
within a few days; fixes ship as a patch release with credit (if wanted).

## Scope: what gritty treats as untrusted

gritty assumes **terminal output, clipboard content, and its own on-disk state
files are hostile** and is hardened accordingly (each with regression tests):

- **PTY/VT output** — escape/OSC parsing via alacritty's engine; OSC-8
  hyperlinks restricted to http/https; bounded parser carries and reply queues;
  synchronized-update buffers capped and deadline-flushed.
- **Clipboard pastes** — control/escape bytes stripped, bracketed-paste end
  markers neutralized, payload size capped.
- **`session.json` / `config.toml`** — size-capped, hand-rolled parsers that
  fail closed; restore caps on windows/tabs/panes (and their product) so a
  crafted session can't mass-spawn shells.
- **Shell resolution** — absolute, trusted paths only; a configured shell is
  honored only when absolute (never resolved via PATH).

`scripts/gate.ps1` (fmt, clippy -D warnings, all tests, size/dependency
budgets; `-Stress` for leak/starvation regression) must pass for every change.

## Supported versions

The latest release only.
