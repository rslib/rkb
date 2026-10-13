# The rkb schedule

`rkb maintain` runs one upkeep pass on the inbox. An OS timer can run it twice a day, at 09:00 and
21:00 local time. The pass pulls proposals, runs the rule triage, expires transcripts, fades unused
lessons and promotes within a daily model cap. Nothing else is needed; the timer does not require a
session.

## Install

```
rkb schedule install
rkb schedule install --uninstall
```

rkb asks you to confirm before it writes anything. An agent reads the question and the user answers
it with `rkb confirm`.

- macOS: `~/Library/LaunchAgents/rkb.maintain.plist`, loaded with `launchctl bootstrap gui/<uid>`.
- Linux: `~/.config/systemd/user/rkb-maintain.service` and
  `~/.config/systemd/user/rkb-maintain.timer`, loaded with
  `systemctl --user enable --now rkb-maintain.timer`.
- Any other system: rkb refuses and names the system.

The job sets `RKB_HOME` and `PATH`, and writes its output to `~/.local/state/rkb/maintain.log`. It
never holds a token. `--uninstall` unloads the job and removes only the files rkb wrote.

## Run one pass now

```
launchctl kickstart -k gui/$(id -u)/rkb.maintain
```

or

```
systemctl --user start rkb-maintain.service
```

A pass also runs by hand:

```
rkb maintain
```

## Check that the background job can reach the model

The timer runs with the environment written into the job, not your shell. A pass from your terminal
does not prove that the job reaches the model, so start the job itself (see above), then run:

```
rkb doctor
```

The `schedule` line of `rkb doctor` names the last pass, a failed pull and the reason the model step
was skipped, such as `no approved model chain` or `the model program failed: exit 1`. On macOS a
background job may not reach the Claude Code login in the Keychain. Run `claude -p "hi"` once in a
terminal to refresh the login, then start the job again. `rkb doctor` also warns about a pass older
than 36 hours.

## A Linux machine where you are often logged out

A systemd user timer stops when your last session ends. Keep it running between sessions:

```
loginctl enable-linger "$USER"
```
