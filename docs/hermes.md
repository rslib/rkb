# rkb in Hermes

`rkb install hermes` puts rkb into Hermes as one MCP server and one skill.
Run it in a terminal.
The command asks for an answer first, because it writes outside the knowledge base.

```bash
rkb install hermes
rkb install hermes --uninstall
```

`rkb install` with no name installs into every detected harness, Hermes included when
`$HERMES_HOME` or `~/.hermes` exists.
`rkb install --list` shows whether Hermes is detected and whether the skill matches this rkb.

## The paths and keys, confirmed from the Hermes source

Each fact below comes from the Hermes repository at <https://github.com/NousResearch/hermes-agent>,
read at these files and lines.

- The config file is `$HERMES_HOME/config.yaml`, or `~/.hermes/config.yaml` by default.
  `hermes_constants.py:1193-1195` (`get_config_path` returns `get_hermes_home() / "config.yaml"`).
- `HERMES_HOME` overrides the folder. The lookup is a context-local override, then the
  `HERMES_HOME` environment variable, then the platform default.
  `hermes_constants.py:111-121` (`get_hermes_home`), and `hermes_constants.py:51-58` for the
  default `Path.home() / ".hermes"`.
- MCP servers live under the top-level `mcp_servers` key, one entry per name, with the keys
  `command`, `args` and `env`.
  `cli-config.yaml.example:1577-1590`, `website/docs/user-guide/features/mcp.md:36-40`, and the
  reader at `hermes_cli/mcp_security.py:80-85`.
- Hermes skills live under `$HERMES_HOME/skills`, one folder per skill with a `SKILL.md` inside.
  `hermes_constants.py:1198-1200` (`get_skills_dir`), and discovery walks the tree for `SKILL.md`
  at `tools/skills_sync.py:87-90`.

## What rkb writes

- `$HERMES_HOME/config.yaml`, as `mcp_servers.rkb`:

  ```yaml
  mcp_servers:
    rkb:
      command: "/absolute/path/to/rkb"
      args: ["mcp", "--tools", "propose"]
      env:
        RKB_AGENT: "hermes"
  ```

  The command is the absolute path of the running rkb binary, so the entry never runs another
  rkb.

  Hermes starts a stdio server with only `PATH`, `HOME`, `USER`, `LANG`, `XDG_*` and a few more
  (`_build_safe_env` in `tools/mcp_tool_config.py`), so other variables never reach rkb.
  The install copies the ones set in its own shell into `env`: `RKB_HOME` (as an absolute path),
  `RKB_OFFLINE=1` and `HF_HUB_OFFLINE=1`.
  Run `rkb install hermes` in the same environment the server uses.

  When `RKB_NTFY_TOKEN` is set, the entry gets only the reference `"${RKB_NTFY_TOKEN}"`, never the
  value. Hermes fills it from its own secrets, such as `$HERMES_HOME/.env`; put the token there.
  When Hermes cannot fill it, rkb sees the unfilled reference, sends no token, and the note stays local.
- `$HERMES_HOME/skills/rkb/SKILL.md`, a short rkb skill.

rkb edits only the `mcp_servers.rkb` lines.
Comments, key order and the user's other servers stay as they are.
The first write copies the old file to `config.yaml.rkb-backup` beside it.

When a `mcp_servers.rkb` entry already exists and names a different command, `args` or `env`, rkb
refuses and writes nothing.
The error says to change the entry by hand so it matches, then run the command again.

## Uninstall

`rkb install hermes --uninstall` removes the `mcp_servers.rkb` entry and the skill folder.
It removes the `mcp_servers` key too when rkb's entry was its only content, so a config rkb
created returns to nothing.

rkb removes the skill only when it still equals the text rkb wrote.
An edited skill stays, and the command prints a `kept ... (edited; remove it by hand)` line.
