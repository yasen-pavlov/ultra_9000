# Security policy

## Supported versions

Only the latest release receives security fixes.

## Reporting a vulnerability

Please report vulnerabilities privately and do not open an issue: use **Report a vulnerability** on the repository's [Security tab](https://github.com/yasen-pavlov/ultra_9000/security/advisories/new) (GitHub private vulnerability reporting).

Include the affected version and platform, reproduction steps and the impact you observed. You will get an acknowledgement as soon as possible, and a fix or mitigation will be released before details are disclosed.

## Scope notes

- Launch inputs (tool, project, folder, resume ids) are treated as data and passed as arguments, never interpreted as shell code. Anything that lets them execute as shell code is in scope.
- Agent session stores (OMP, Claude Code, Codex, pi) are read only, with bounded reads that never load message content. Any write to them, or any read beyond that, is a bug.
- Global Alacritty, Neovim and agent configuration are never modified; only launch-scoped environment variables are set.
- User icon files are validated and confined to the config directory; escaping it is in scope.
- Vulnerabilities in vendored Alacritty code should also be reported upstream when they are not specific to this fork.
