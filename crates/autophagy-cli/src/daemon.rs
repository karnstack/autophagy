//! Daemon lifecycle (`autophagy daemon install|uninstall|status`).
//!
//! Generates and installs a platform supervisor unit that runs `autophagy
//! watch` in the background: a launchd user agent on macOS, a systemd user unit
//! on Linux. The unit contents and the reversible, marker-guarded write/remove
//! discipline live in `autophagy-install`; this module resolves paths, renders
//! reports, and drives `launchctl`/`systemctl` through the [`Supervisor`] shim
//! so tests never need a real init system.
//!
//! The daemon only ingests. Installing the unit is explicit user opt-in and adds
//! no autonomous execution: the watch process it launches never executes or
//! installs anything.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

use autophagy_install::{
    SupervisorConfig, SupervisorKind, SupervisorPlan, plan_supervisor, remove_supervisor,
    write_supervisor,
};
use serde::Serialize;

use crate::{CliError, resolve_database_path};

/// Thin shim over the platform init system so lifecycle logic is testable
/// without a live launchd/systemd.
pub trait Supervisor {
    /// Load/bootstrap the job so it runs now and at login.
    fn load(&self, plan: &SupervisorPlan) -> Result<(), CliError>;
    /// Unload/bootout the job.
    fn unload(&self, plan: &SupervisorPlan) -> Result<(), CliError>;
    /// Whether the init system currently knows about the job.
    fn is_loaded(&self, plan: &SupervisorPlan) -> Result<bool, CliError>;
}

/// macOS `launchctl` supervisor (uses the portable `load`/`unload`/`list` verbs
/// so no session/UID domain target is required).
struct LaunchctlSupervisor;

impl Supervisor for LaunchctlSupervisor {
    fn load(&self, plan: &SupervisorPlan) -> Result<(), CliError> {
        run_command(
            "launchctl",
            &[
                "load".into(),
                "-w".into(),
                plan.unit_path.to_string_lossy().into_owned(),
            ],
        )
    }

    fn unload(&self, plan: &SupervisorPlan) -> Result<(), CliError> {
        run_command(
            "launchctl",
            &[
                "unload".into(),
                "-w".into(),
                plan.unit_path.to_string_lossy().into_owned(),
            ],
        )
    }

    fn is_loaded(&self, plan: &SupervisorPlan) -> Result<bool, CliError> {
        Ok(command_succeeds("launchctl", &["list".into(), plan.label.clone()]))
    }
}

/// Linux `systemctl --user` supervisor.
struct SystemctlSupervisor;

impl SystemctlSupervisor {
    const UNIT: &'static str = "autophagy-watch.service";
}

impl Supervisor for SystemctlSupervisor {
    fn load(&self, _plan: &SupervisorPlan) -> Result<(), CliError> {
        run_command("systemctl", &["--user".into(), "daemon-reload".into()])?;
        run_command(
            "systemctl",
            &[
                "--user".into(),
                "enable".into(),
                "--now".into(),
                Self::UNIT.into(),
            ],
        )
    }

    fn unload(&self, _plan: &SupervisorPlan) -> Result<(), CliError> {
        run_command(
            "systemctl",
            &[
                "--user".into(),
                "disable".into(),
                "--now".into(),
                Self::UNIT.into(),
            ],
        )
    }

    fn is_loaded(&self, _plan: &SupervisorPlan) -> Result<bool, CliError> {
        Ok(command_succeeds(
            "systemctl",
            &["--user".into(), "is-active".into(), Self::UNIT.into()],
        ))
    }
}

fn run_command(program: &str, args: &[String]) -> Result<(), CliError> {
    let status = Command::new(program).args(args).status().map_err(|error| {
        CliError::SupervisorCommand {
            command: format!("{program} {}", args.join(" ")),
            detail: error.to_string(),
        }
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::SupervisorCommand {
            command: format!("{program} {}", args.join(" ")),
            detail: format!("exited with {status}"),
        })
    }
}

fn command_succeeds(program: &str, args: &[String]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Resolved filesystem environment for the current host.
pub struct DaemonEnv {
    /// Supervisor flavour for this host.
    pub kind: SupervisorKind,
    /// Directory the unit file lives in.
    pub unit_directory: PathBuf,
    /// Absolute path to the running `autophagy` binary.
    pub binary_path: PathBuf,
    /// Watch cycle interval in seconds.
    pub interval_secs: u64,
    /// Native adapter names passed to `watch --adapter`.
    pub adapters: Vec<String>,
    /// Explicit database path recorded in the unit.
    pub database: Option<PathBuf>,
    /// Standard-output log path.
    pub log_path: PathBuf,
    /// Standard-error log path.
    pub error_log_path: PathBuf,
}

impl DaemonEnv {
    fn config(&self) -> SupervisorConfig {
        SupervisorConfig {
            kind: self.kind,
            unit_directory: self.unit_directory.clone(),
            binary_path: self.binary_path.clone(),
            interval_secs: self.interval_secs,
            adapters: self.adapters.clone(),
            database: self.database.clone(),
            log_path: self.log_path.clone(),
            error_log_path: self.error_log_path.clone(),
        }
    }
}

/// Supervisor flavour for the host, or `None` when unsupported.
fn host_kind() -> Option<SupervisorKind> {
    if cfg!(target_os = "macos") {
        Some(SupervisorKind::Launchd)
    } else if cfg!(target_os = "linux") {
        Some(SupervisorKind::Systemd)
    } else {
        None
    }
}

fn real_supervisor(kind: SupervisorKind) -> Box<dyn Supervisor> {
    match kind {
        SupervisorKind::Launchd => Box::new(LaunchctlSupervisor),
        SupervisorKind::Systemd => Box::new(SystemctlSupervisor),
    }
}

fn home_dir() -> Result<PathBuf, CliError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
        .ok_or(CliError::HomeDirectoryUnavailable)
}

/// Pure unit-directory resolution, so tests can inject a temporary home without
/// mutating process environment (which is `unsafe` and forbidden here).
fn unit_directory(kind: SupervisorKind, home: &Path, xdg_config_home: Option<PathBuf>) -> PathBuf {
    match kind {
        SupervisorKind::Launchd => home.join("Library").join("LaunchAgents"),
        SupervisorKind::Systemd => {
            let base = xdg_config_home.unwrap_or_else(|| home.join(".config"));
            base.join("systemd").join("user")
        }
    }
}

fn host_unit_directory(kind: SupervisorKind) -> Result<PathBuf, CliError> {
    let home = home_dir()?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty());
    Ok(unit_directory(kind, &home, xdg))
}

fn app_data_dir() -> Result<PathBuf, CliError> {
    let project = directories::ProjectDirs::from("sh", "autophagy", "Autophagy")
        .ok_or(CliError::DataDirectoryUnavailable)?;
    Ok(project.data_local_dir().to_path_buf())
}

/// Resolve the daemon environment from the real host, or `None` if unsupported.
fn resolve_env(
    database: Option<PathBuf>,
    interval_secs: u64,
    adapters: Vec<String>,
) -> Result<Option<DaemonEnv>, CliError> {
    let Some(kind) = host_kind() else {
        return Ok(None);
    };
    let data_dir = app_data_dir()?;
    Ok(Some(DaemonEnv {
        kind,
        unit_directory: host_unit_directory(kind)?,
        binary_path: std::env::current_exe()?,
        interval_secs,
        adapters,
        database: Some(resolve_database_path(database)?),
        log_path: data_dir.join("watch.log"),
        error_log_path: data_dir.join("watch.err.log"),
    }))
}

/// Read the last non-empty line of the watch log, if present.
fn last_log_line(log_path: &PathBuf) -> Option<String> {
    let contents = fs::read_to_string(log_path).ok()?;
    contents
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::to_owned)
}

/// Serializable daemon command result.
#[derive(Debug, Serialize)]
pub struct DaemonReport {
    /// `install`, `uninstall`, or `status`.
    pub action: &'static str,
    /// `launchd`, `systemd`, or `unsupported`.
    pub platform: &'static str,
    /// Whether the current host is supported.
    pub supported: bool,
    /// Job label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Absolute unit path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_path: Option<String>,
    /// Exact command line the unit launches (install only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub program_arguments: Vec<String>,
    /// Whether the unit file is present on disk.
    pub unit_present: bool,
    /// Whether the init system reports the job loaded, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_loaded: Option<bool>,
    /// Whether uninstall removed a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<bool>,
    /// Log destination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    /// Last watch cycle line from the log tail, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<String>,
    /// Human-readable guidance (e.g. on unsupported hosts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl DaemonReport {
    fn unsupported(action: &'static str) -> Self {
        Self {
            action,
            platform: "unsupported",
            supported: false,
            label: None,
            unit_path: None,
            program_arguments: Vec::new(),
            unit_present: false,
            job_loaded: None,
            removed: None,
            log_path: None,
            last_cycle: None,
            message: Some(
                "daemon lifecycle is not supported on this platform yet; run `autophagy watch` under your own process supervisor".to_owned(),
            ),
        }
    }

    const fn platform_name(kind: SupervisorKind) -> &'static str {
        match kind {
            SupervisorKind::Launchd => "launchd",
            SupervisorKind::Systemd => "systemd",
        }
    }
}

/// `daemon install`.
pub fn install(
    database: Option<PathBuf>,
    interval_secs: u64,
    adapters: Vec<String>,
) -> Result<DaemonReport, CliError> {
    let Some(env) = resolve_env(database, interval_secs, adapters)? else {
        return Ok(DaemonReport::unsupported("install"));
    };
    let supervisor = real_supervisor(env.kind);
    install_with(&env, supervisor.as_ref())
}

/// `daemon uninstall`.
pub fn uninstall(database: Option<PathBuf>) -> Result<DaemonReport, CliError> {
    let Some(env) = resolve_env(database, default_interval(), default_adapters())? else {
        return Ok(DaemonReport::unsupported("uninstall"));
    };
    let supervisor = real_supervisor(env.kind);
    uninstall_with(&env, supervisor.as_ref())
}

/// `daemon status`.
pub fn status(database: Option<PathBuf>) -> Result<DaemonReport, CliError> {
    let Some(env) = resolve_env(database, default_interval(), default_adapters())? else {
        return Ok(DaemonReport::unsupported("status"));
    };
    let supervisor = real_supervisor(env.kind);
    status_with(&env, supervisor.as_ref())
}

fn default_interval() -> u64 {
    60
}

fn default_adapters() -> Vec<String> {
    crate::watch::NativeAdapter::ALL
        .iter()
        .map(|adapter| adapter.as_str().to_owned())
        .collect()
}

fn install_with(env: &DaemonEnv, supervisor: &dyn Supervisor) -> Result<DaemonReport, CliError> {
    let plan = plan_supervisor(&env.config());
    if let Some(parent) = env.log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_supervisor(&plan)?;
    supervisor.load(&plan)?;
    let job_loaded = supervisor.is_loaded(&plan).ok();
    Ok(DaemonReport {
        action: "install",
        platform: DaemonReport::platform_name(env.kind),
        supported: true,
        label: Some(plan.label.clone()),
        unit_path: Some(plan.unit_path.to_string_lossy().into_owned()),
        program_arguments: plan.program_arguments.clone(),
        unit_present: plan.unit_path.exists(),
        job_loaded,
        removed: None,
        log_path: Some(env.log_path.to_string_lossy().into_owned()),
        last_cycle: None,
        message: None,
    })
}

fn uninstall_with(env: &DaemonEnv, supervisor: &dyn Supervisor) -> Result<DaemonReport, CliError> {
    let plan = plan_supervisor(&env.config());
    // Best-effort unload before removing the file.
    if supervisor.is_loaded(&plan).unwrap_or(false) {
        supervisor.unload(&plan)?;
    }
    let removed = remove_supervisor(&plan.unit_path)?;
    Ok(DaemonReport {
        action: "uninstall",
        platform: DaemonReport::platform_name(env.kind),
        supported: true,
        label: Some(plan.label.clone()),
        unit_path: Some(plan.unit_path.to_string_lossy().into_owned()),
        program_arguments: Vec::new(),
        unit_present: plan.unit_path.exists(),
        job_loaded: None,
        removed: Some(removed),
        log_path: None,
        last_cycle: None,
        message: None,
    })
}

// Parallels `install_with`/`uninstall_with`; kept fallible for a uniform shim
// signature even though status probing never errors today.
#[allow(clippy::unnecessary_wraps)]
fn status_with(env: &DaemonEnv, supervisor: &dyn Supervisor) -> Result<DaemonReport, CliError> {
    let plan = plan_supervisor(&env.config());
    let unit_present = plan.unit_path.exists();
    let job_loaded = supervisor.is_loaded(&plan).ok();
    Ok(DaemonReport {
        action: "status",
        platform: DaemonReport::platform_name(env.kind),
        supported: true,
        label: Some(plan.label.clone()),
        unit_path: Some(plan.unit_path.to_string_lossy().into_owned()),
        program_arguments: Vec::new(),
        unit_present,
        job_loaded,
        removed: None,
        log_path: Some(env.log_path.to_string_lossy().into_owned()),
        last_cycle: last_log_line(&env.log_path),
        message: None,
    })
}

/// Render a daemon report as human-readable text.
pub fn write_text(report: &DaemonReport, writer: &mut impl Write) -> io::Result<()> {
    if !report.supported {
        if let Some(message) = &report.message {
            writeln!(writer, "{message}")?;
        }
        return Ok(());
    }
    match report.action {
        "install" => {
            writeln!(
                writer,
                "installed {} unit for {}",
                report.platform,
                report.label.as_deref().unwrap_or("sh.autophagy.watch")
            )?;
            if let Some(path) = &report.unit_path {
                writeln!(writer, "wrote {path}")?;
            }
            writeln!(writer, "runs: {}", report.program_arguments.join(" "))?;
            if let Some(log) = &report.log_path {
                writeln!(writer, "logs: {log}")?;
            }
            writeln!(
                writer,
                "loaded: {}",
                describe_loaded(report.job_loaded)
            )?;
        }
        "uninstall" => {
            writeln!(
                writer,
                "uninstalled {} unit; file removed: {}",
                report.platform,
                report.removed.unwrap_or(false)
            )?;
            if let Some(path) = &report.unit_path {
                writeln!(writer, "path: {path}")?;
            }
        }
        _ => {
            writeln!(
                writer,
                "{} unit present: {} · loaded: {}",
                report.platform,
                report.unit_present,
                describe_loaded(report.job_loaded)
            )?;
            if let Some(path) = &report.unit_path {
                writeln!(writer, "path: {path}")?;
            }
            if let Some(log) = &report.log_path {
                writeln!(writer, "logs: {log}")?;
            }
            if let Some(last) = &report.last_cycle {
                writeln!(writer, "last log line: {last}")?;
            }
        }
    }
    Ok(())
}

fn describe_loaded(loaded: Option<bool>) -> &'static str {
    match loaded {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records lifecycle calls and reports a controllable loaded state.
    struct FakeSupervisor {
        loaded: RefCell<bool>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeSupervisor {
        fn new() -> Self {
            Self {
                loaded: RefCell::new(false),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Supervisor for FakeSupervisor {
        fn load(&self, _plan: &SupervisorPlan) -> Result<(), CliError> {
            self.calls.borrow_mut().push("load".to_owned());
            *self.loaded.borrow_mut() = true;
            Ok(())
        }

        fn unload(&self, _plan: &SupervisorPlan) -> Result<(), CliError> {
            self.calls.borrow_mut().push("unload".to_owned());
            *self.loaded.borrow_mut() = false;
            Ok(())
        }

        fn is_loaded(&self, _plan: &SupervisorPlan) -> Result<bool, CliError> {
            Ok(*self.loaded.borrow())
        }
    }

    fn temp_env(kind: SupervisorKind) -> (tempfile::TempDir, DaemonEnv) {
        let dir = tempfile::tempdir().expect("temp dir");
        let env = DaemonEnv {
            kind,
            unit_directory: dir.path().join("units"),
            binary_path: PathBuf::from("/opt/autophagy/bin/autophagy"),
            interval_secs: 60,
            adapters: default_adapters(),
            database: Some(PathBuf::from("/tmp/demo.db")),
            log_path: dir.path().join("data").join("watch.log"),
            error_log_path: dir.path().join("data").join("watch.err.log"),
        };
        (dir, env)
    }

    #[test]
    fn install_status_uninstall_round_trip() {
        let (_dir, env) = temp_env(SupervisorKind::Launchd);
        let supervisor = FakeSupervisor::new();

        let installed = install_with(&env, &supervisor).expect("install");
        assert_eq!(installed.action, "install");
        assert!(installed.unit_present);
        assert_eq!(installed.job_loaded, Some(true));
        let unit_path = PathBuf::from(installed.unit_path.clone().expect("path"));
        assert!(unit_path.exists());
        assert!(installed.program_arguments.contains(&"watch".to_owned()));

        let status = status_with(&env, &supervisor).expect("status");
        assert!(status.unit_present);
        assert_eq!(status.job_loaded, Some(true));

        let uninstalled = uninstall_with(&env, &supervisor).expect("uninstall");
        assert_eq!(uninstalled.removed, Some(true));
        assert!(!unit_path.exists());
        assert_eq!(supervisor.calls.borrow().as_slice(), ["load", "unload"]);
    }

    #[test]
    fn install_refuses_a_foreign_unit_file() {
        let (_dir, env) = temp_env(SupervisorKind::Launchd);
        let supervisor = FakeSupervisor::new();
        let plan = plan_supervisor(&env.config());
        fs::create_dir_all(&env.unit_directory).expect("mkdir");
        fs::write(&plan.unit_path, "not ours\n").expect("seed foreign");

        let error = install_with(&env, &supervisor).expect_err("must refuse");
        assert!(matches!(error, CliError::Supervisor(_)));
        // The foreign file is untouched and the job was never loaded.
        assert_eq!(fs::read_to_string(&plan.unit_path).unwrap(), "not ours\n");
        assert!(supervisor.calls.borrow().is_empty());
    }

    #[test]
    fn status_reports_absent_unit_before_install() {
        let (_dir, env) = temp_env(SupervisorKind::Systemd);
        let supervisor = FakeSupervisor::new();
        let status = status_with(&env, &supervisor).expect("status");
        assert!(!status.unit_present);
        assert_eq!(status.job_loaded, Some(false));
    }

    #[test]
    fn status_surfaces_last_log_line() {
        let (_dir, env) = temp_env(SupervisorKind::Launchd);
        let supervisor = FakeSupervisor::new();
        fs::create_dir_all(env.log_path.parent().unwrap()).expect("mkdir");
        fs::write(&env.log_path, "old line\nlast cycle line\n\n").expect("log");
        let status = status_with(&env, &supervisor).expect("status");
        assert_eq!(status.last_cycle.as_deref(), Some("last cycle line"));
    }

    #[test]
    fn unit_directory_resolves_under_a_temporary_home() {
        // A temp home stands in for HOME so tests never touch the real
        // ~/Library/LaunchAgents or ~/.config.
        let home = PathBuf::from("/tmp/fake-home");
        assert_eq!(
            unit_directory(SupervisorKind::Launchd, &home, None),
            home.join("Library").join("LaunchAgents")
        );
        assert_eq!(
            unit_directory(SupervisorKind::Systemd, &home, None),
            home.join(".config").join("systemd").join("user")
        );
        assert_eq!(
            unit_directory(SupervisorKind::Systemd, &home, Some(PathBuf::from("/xdg"))),
            PathBuf::from("/xdg").join("systemd").join("user")
        );
    }
}
