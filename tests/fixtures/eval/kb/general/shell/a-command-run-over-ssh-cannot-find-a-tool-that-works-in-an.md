---
schema: 1
id: 2b00000022
type: pitfall
status: active
verified: 2026-07-11
verified_how: read
tags:
  - shell
  - ssh
  - non-interactive-shell
---

# A command run over ssh cannot find a tool that works in an interactive login

## Symptom
```text
$ ssh host 'mytool --version'
bash: mytool: command not found
```
but logging in interactively with `ssh host` and then running `mytool --version` works fine.

## Cause
`ssh host 'command'` starts a non-interactive, non-login shell on the remote side. Bash only sources `~/.bashrc` for interactive shells, and only sources `~/.bash_profile` (or `/etc/profile`) for login shells; a script that adds `mytool` to `PATH` inside `.bashrc` behind an `if [[ $- == *i* ]]` guard, or only in `.bash_profile`, never runs for a bare remote command.

## Fix
Either export `PATH` unconditionally near the top of `.bashrc` (before any interactive-only guard), or invoke the remote command through a login shell explicitly:

```sh
ssh host 'bash -lc "mytool --version"'
```

## Evidence
Forcing a login shell with `bash -lc` picked up the same `PATH` as an interactive session and found `mytool` without editing any remote dotfiles.
