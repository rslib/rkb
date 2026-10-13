//! The OS timer that runs `rkb maintain` twice a day: a launchd agent on macOS, a catch-up systemd
//! user timer on Linux. Both carry `RKB_HOME` and `PATH`, never a secret.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::install::{Change, Effect};

/// The job label, shared by launchd and systemd.
pub const LABEL: &str = "rkb.maintain";
/// The local hours the job runs.
pub const HOURS: [u8; 2] = [9, 21];

/// The marker every job file carries, so `--uninstall` removes only what rkb wrote.
pub const MARK: &str = "written by rkb schedule install";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Macos,
    Linux,
    Other,
}

pub fn current_os() -> Os {
    match std::env::consts::OS {
        "macos" => Os::Macos,
        "linux" => Os::Linux,
        _ => Os::Other,
    }
}

pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

pub fn service_path(home: &Path) -> PathBuf {
    home.join(".config/systemd/user/rkb-maintain.service")
}

pub fn timer_path(home: &Path) -> PathBuf {
    home.join(".config/systemd/user/rkb-maintain.timer")
}

/// The file a person confirms: the launchd agent on macOS, the systemd service elsewhere.
pub fn primary_path(home: &Path) -> PathBuf {
    match current_os() {
        Os::Macos => plist_path(home),
        _ => service_path(home),
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The launchd agent text: the absolute rkb binary, `maintain`, both hours, the environment and the log.
pub fn plist(binary: &Path, rkb_home: &Path, path: &str, log: &Path) -> String {
    let mut cal = String::new();
    for h in HOURS {
        cal.push_str(&format!("    <dict>\n      <key>Hour</key>\n      <integer>{h}</integer>\n      <key>Minute</key>\n      <integer>0</integer>\n    </dict>\n"));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<!-- {MARK}; rkb schedule install --uninstall removes this file -->\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key>\n  <string>{LABEL}</string>\n\
  <key>ProgramArguments</key>\n  <array>\n    <string>{bin}</string>\n    <string>maintain</string>\n  </array>\n\
  <key>EnvironmentVariables</key>\n  <dict>\n    <key>RKB_HOME</key>\n    <string>{home}</string>\n    <key>PATH</key>\n    <string>{path}</string>\n  </dict>\n\
  <key>StartCalendarInterval</key>\n  <array>\n{cal}  </array>\n\
  <key>StandardOutPath</key>\n  <string>{log}</string>\n\
  <key>StandardErrorPath</key>\n  <string>{log}</string>\n\
</dict>\n</plist>\n",
        bin = xml_escape(&binary.display().to_string()),
        home = xml_escape(&rkb_home.display().to_string()),
        path = xml_escape(path),
        log = xml_escape(&log.display().to_string()),
    )
}

/// The systemd user service text.
pub fn service(binary: &Path, rkb_home: &Path, path: &str, log: &Path) -> String {
    format!(
        "# {MARK}; rkb schedule install --uninstall removes this file\n\
[Unit]\n\
Description=rkb inbox upkeep\n\
\n\
[Service]\n\
Type=oneshot\n\
ExecStart={bin} maintain\n\
Environment=\"RKB_HOME={home}\"\n\
Environment=\"PATH={path}\"\n\
StandardOutput=append:{log}\n\
StandardError=append:{log}\n",
        bin = binary.display(),
        home = rkb_home.display(),
        log = log.display(),
    )
}

/// The systemd user timer text: both local times and a catch-up run after a slept machine.
pub fn timer() -> String {
    let times = HOURS.map(|h| format!("{h:02}")).join(",");
    format!(
        "# {MARK}; rkb schedule install --uninstall removes this file\n\
[Unit]\n\
Description=Run rkb maintain twice a day\n\
\n\
[Timer]\n\
OnCalendar=*-*-* {times}:00:00\n\
Persistent=true\n\
\n\
[Install]\n\
WantedBy=timers.target\n"
    )
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn state_log() -> PathBuf {
    crate::paths::state_dir().join("maintain.log")
}

fn binary() -> Result<PathBuf> {
    std::env::current_exe().map_err(|e| Error::Refused(format!("cannot find the rkb binary: {e}")))
}

fn rkb_home() -> PathBuf {
    crate::kb::home()
}

fn path_env() -> String {
    std::env::var("PATH").unwrap_or_default()
}

fn uid() -> String {
    let out = Command::new("id").arg("-u").output();
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0".into())
}

fn write_atomic(path: &Path, text: &str) -> Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(false);
    }
    let dir = path.parent().expect("absolute path");
    std::fs::create_dir_all(dir).map_err(crate::error::io(dir))?;
    let tmp = dir.join(format!(".{}.rkb-tmp", path.file_name().unwrap().to_string_lossy()));
    std::fs::write(&tmp, text).map_err(crate::error::io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(crate::error::io(path))?;
    Ok(true)
}

/// Removes `path` only when it holds rkb's marker. Returns whether it removed the file.
fn remove_ours(path: &Path) -> bool {
    if std::fs::read_to_string(path).is_ok_and(|t| t.contains(MARK)) {
        return std::fs::remove_file(path).is_ok();
    }
    false
}

/// Runs a command, ignoring a failure (an unload that has nothing to unload).
fn run(program: &str, args: &[&str]) -> bool {
    Command::new(program).args(args).output().is_ok_and(|o| o.status.success())
}

/// Runs a command and fails with its own name when it does not run.
fn run_checked(program: &str, args: &[&str]) -> Result<()> {
    match Command::new(program).args(args).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(Error::Refused(format!("`{program} {}` failed: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()))),
        Err(e) => Err(Error::Refused(format!("cannot run `{program}`: {e}"))),
    }
}

/// Installs or removes the OS job. Only files with rkb's marker are removed, and the job is unloaded first.
pub fn install(uninstall: bool) -> Result<Vec<Change>> {
    install_os(current_os(), uninstall)
}

fn install_os(os: Os, uninstall: bool) -> Result<Vec<Change>> {
    match os {
        Os::Macos => macos(uninstall),
        Os::Linux => linux(uninstall),
        Os::Other => Err(Error::Refused(format!(
            "the OS schedule supports macOS (launchd) and Linux (systemd user units) only; this host is {}",
            std::env::consts::OS
        ))),
    }
}

fn macos(uninstall: bool) -> Result<Vec<Change>> {
    let path = plist_path(&home());
    if uninstall {
        let _ = run("launchctl", &["bootout", &format!("gui/{}/{}", uid(), LABEL)]);
        let effect = if remove_ours(&path) { Effect::Removed } else { Effect::Unchanged };
        return Ok(vec![Change { path, effect }]);
    }
    let text = plist(&binary()?, &rkb_home(), &path_env(), &state_log());
    let effect = if write_atomic(&path, &text)? {
        let domain = format!("gui/{}", uid());
        // A loaded job keeps its old file, and bootstrap refuses a label that is already loaded.
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        run_checked("launchctl", &["bootstrap", &domain, &path.display().to_string()])?;
        Effect::Written
    } else {
        Effect::Unchanged
    };
    Ok(vec![Change { path, effect }])
}

fn linux(uninstall: bool) -> Result<Vec<Change>> {
    let home = home();
    let (service_file, timer_file) = (service_path(&home), timer_path(&home));
    if uninstall {
        let unit = timer_file.file_name().unwrap().to_string_lossy().to_string();
        let _ = run("systemctl", &["--user", "disable", "--now", &unit]);
        let _ = run("systemctl", &["--user", "daemon-reload"]);
        let mut out = vec![];
        for path in [service_file, timer_file] {
            let effect = if remove_ours(&path) { Effect::Removed } else { Effect::Unchanged };
            out.push(Change { path, effect });
        }
        return Ok(out);
    }
    let service_text = service(&binary()?, &rkb_home(), &path_env(), &state_log());
    let wrote_service = write_atomic(&service_file, &service_text)?;
    let wrote_timer = write_atomic(&timer_file, &timer())?;
    if wrote_service || wrote_timer {
        run_checked("systemctl", &["--user", "daemon-reload"])?;
        run_checked("systemctl", &["--user", "enable", "--now", "rkb-maintain.timer"])?;
    }
    Ok(vec![
        Change { path: service_file, effect: if wrote_service { Effect::Written } else { Effect::Unchanged } },
        Change { path: timer_file, effect: if wrote_timer { Effect::Written } else { Effect::Unchanged } },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launchd_agent_names_the_command_the_times_and_the_environment() {
        let text = plist(
            Path::new("/usr/local/bin/rkb"),
            Path::new("/home/u/kb"),
            "/usr/local/bin:/usr/bin",
            Path::new("/home/u/.local/state/rkb/maintain.log"),
        );
        assert!(text.contains("<key>Label</key>\n  <string>rkb.maintain</string>"), "{text}");
        assert!(text.contains("<string>/usr/local/bin/rkb</string>\n    <string>maintain</string>"), "{text}");
        assert!(text.contains("<key>RKB_HOME</key>\n    <string>/home/u/kb</string>"), "{text}");
        assert!(text.contains("<key>PATH</key>\n    <string>/usr/local/bin:/usr/bin</string>"), "{text}");
        assert!(text.contains("<key>StartCalendarInterval</key>"), "{text}");
        assert!(text.contains("<integer>9</integer>") && text.contains("<integer>21</integer>"), "{text}");
        assert!(text.contains("<key>StandardOutPath</key>\n  <string>/home/u/.local/state/rkb/maintain.log</string>"), "{text}");
        assert!(text.contains(MARK), "the ownership marker is present: {text}");
        assert!(!text.contains("TOKEN") && !text.contains("token="), "no secret in the job file");
    }

    #[test]
    fn the_systemd_units_carry_both_times_and_persistent() {
        let service = service(
            Path::new("/usr/bin/rkb"),
            Path::new("/home/u/kb"),
            "/usr/bin:/bin",
            Path::new("/home/u/.local/state/rkb/maintain.log"),
        );
        assert!(service.contains("ExecStart=/usr/bin/rkb maintain"), "{service}");
        assert!(service.contains("Environment=\"RKB_HOME=/home/u/kb\""), "{service}");
        assert!(service.contains("Environment=\"PATH=/usr/bin:/bin\""), "{service}");
        assert!(service.contains(MARK), "{service}");
        let timer = timer();
        assert!(timer.contains("OnCalendar=*-*-* 09,21:00:00"), "{timer}");
        assert!(timer.contains("Persistent=true"), "{timer}");
        assert!(timer.contains("WantedBy=timers.target"), "{timer}");
        assert!(timer.contains(MARK), "{timer}");
    }

    #[test]
    fn another_os_is_refused_by_name() {
        let Err(e) = install_os(Os::Other, false) else { panic!("another OS is refused") };
        assert!(matches!(e, Error::Refused(ref m) if m.contains("macOS") && m.contains("Linux")), "{e}");
    }
}
