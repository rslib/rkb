# Running rkb on a server

A short guide for an owner who runs rkb in a container on a server, with no network egress.

## The container volume

Keep the knowledge base and rkb's per-machine state on one volume, so a rebuilt container keeps both:

```bash
docker run -v kb-volume:/data ...
```

Set `RKB_HOME=/data/kb` (the knowledge base) and `HOME=/data` (state, config and cache live under `$HOME`). Nothing else is needed; rkb never writes outside these folders and `/tmp`. With `RKB_HOME` and no `HOME`, the state goes in `$RKB_HOME/.git/rkb-home`, which git never syncs.

## The variables

- `RKB_HOME`: the knowledge base folder. Required when `HOME` is not set; without it and without `HOME`, rkb stops and writes nothing.
- `RKB_AGENT`: the name of the agent that runs on the server, 1 to 32 characters from `a-z`, `0-9`, `_` and `-`, so every machine can tell which agent used a lesson.
- `RKB_NTFY_TOKEN`: the bearer token for the ntfy topic in `[proposals] ntfy` when it needs one. It is read from the environment and never written down.

## Git sync through a proxy

The server cannot reach `ssh` remotes, so sync over HTTPS through the proxy:

```bash
export HTTPS_PROXY=http://proxy.example.org:3128
```

Give git the read-write token from the environment, so `rkb sync` needs no person and the token is not written to the git config:

```bash
git config --global credential.helper \
  '!f() { echo username=token; echo "password=$KB_GIT_TOKEN"; }; f'
```

Use a token that can read and push the knowledge base repository at `https://example.org/owner/kb.git`. Then `rkb sync` pulls, checks and pushes over HTTPS.

## Agents on the server

Serve agents the read-and-propose tool set, so they can search and note but not write lessons:

```bash
rkb mcp --tools propose
```

`rkb doctor` shows the server state, including which process holds the write lock, with its pid and host.
