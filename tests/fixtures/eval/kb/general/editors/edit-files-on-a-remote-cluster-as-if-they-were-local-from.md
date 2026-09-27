---
schema: 1
id: 2b00000057
type: recipe
status: active
verified: 2026-01-02
verified_how: ran
tags:
  - vscode
  - remote-ssh
  - cluster
---

# Edit files on a remote cluster as if they were local, from VS Code

## When to use
When editing source files on a remote server or HPC login node currently means editing locally and re-uploading, or editing directly over an ssh terminal session without syntax highlighting or a file tree.

## Steps
Install the "Remote - SSH" extension, then connect through an existing `~/.ssh/config` host entry:

```text
# Command Palette
Remote-SSH: Connect to Host... -> select the configured host
```
VS Code opens a window backed by a small server process it installs on the remote machine; the file tree, terminal, and extensions then all operate on the remote filesystem directly.

## Evidence
After connecting, opening a project folder on the remote login node showed the same file tree and syntax highlighting as a local project, and edits saved directly to the remote disk without any manual `scp` step.
