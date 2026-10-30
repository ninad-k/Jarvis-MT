//! Windows service: `jarvis-api service install|uninstall|start|stop|run`.
//!
//! `install` registers this executable with the arguments
//! `--config <abs> service run --workdir <abs>`, so the service finds `config/`, `data/`,
//! `models/` and `.env` (loaded from the working directory) the same way a console run does.
//! It starts automatically after boot (delayed) and restarts itself after a failure.
//!
//! The service runs as the virtual account `NT SERVICE\JarvisMT` (SVC-1): Windows creates it
//! for the service, it has no password and no rights beyond an ordinary service, so a flaw in
//! the server does not run as SYSTEM. `install` prints the `icacls` commands that give it
//! access to exactly what it needs. `--account` picks another account, `--local-system`
//! restores LocalSystem (with a warning).
//! `run` is what the Service Control Manager launches: it logs to `logs/` and stops
//! gracefully on a stop or shutdown request.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::cli::ServiceCommand;
use crate::config::Config;

/// The account a service runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunAs {
    /// `NT SERVICE\JarvisMT`: made by Windows, no password. The default.
    VirtualAccount,
    /// LocalSystem: unrestricted. Only with `--local-system`.
    LocalSystem,
    /// Any other account; `password` is `None` for accounts that need none.
    Account {
        name: String,
        password: Option<String>,
    },
}

/// The virtual service account of this service: `NT SERVICE\JarvisMT`.
pub fn virtual_account() -> String {
    format!("NT SERVICE\\{SERVICE_NAME}")
}

impl RunAs {
    /// From the `install` flags. The flags exclude each other (clap enforces it too).
    pub fn from_flags(
        account: Option<String>,
        password: Option<String>,
        local_system: bool,
    ) -> anyhow::Result<Self> {
        match (account, local_system) {
            (Some(_), true) => Err(anyhow!("--account and --local-system exclude each other")),
            (None, true) => Ok(Self::LocalSystem),
            (Some(name), false) if name.trim().is_empty() => {
                Err(anyhow!("--account needs an account name"))
            }
            (Some(name), false) => Ok(Self::Account {
                name: name.trim().to_owned(),
                password,
            }),
            (None, false) if password.is_some() => {
                Err(anyhow!("a password only makes sense with --account"))
            }
            (None, false) => Ok(Self::VirtualAccount),
        }
    }

    /// `ServiceInfo::account_name`: `None` means LocalSystem.
    pub fn account_name(&self) -> Option<OsString> {
        match self {
            Self::VirtualAccount => Some(OsString::from(virtual_account())),
            Self::LocalSystem => None,
            Self::Account { name, .. } => Some(OsString::from(name)),
        }
    }

    /// `ServiceInfo::account_password`: none for the virtual account and for LocalSystem.
    pub fn account_password(&self) -> Option<OsString> {
        match self {
            Self::Account {
                password: Some(p), ..
            } => Some(OsString::from(p)),
            _ => None,
        }
    }

    /// The name to put in `icacls` commands; `None` for LocalSystem, which needs no grants.
    pub fn grantee(&self) -> Option<String> {
        match self {
            Self::VirtualAccount => Some(virtual_account()),
            Self::LocalSystem => None,
            Self::Account { name, .. } => Some(name.clone()),
        }
    }
}

/// Everything `service install` registers, as one value (so it can be checked without
/// installing anything).
pub fn service_info(exe: PathBuf, workdir: &Path, config: &Path, run_as: &RunAs) -> ServiceInfo {
    ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: launch_arguments(workdir, config),
        dependencies: vec![],
        account_name: run_as.account_name(),
        account_password: run_as.account_password(),
    }
}

/// What the service has to be able to open. `None` entries do not exist on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessPlan {
    pub exe: PathBuf,
    pub workdir: PathBuf,
    pub config: PathBuf,
    pub env_file: Option<PathBuf>,
    pub models_dir: Option<PathBuf>,
    pub static_dir: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
}

impl AccessPlan {
    /// Look at the working directory and the configuration. A configuration that cannot be
    /// read only means the key file and the web UI folder are not listed.
    pub fn discover(exe: &Path, workdir: &Path, config_path: &Path) -> Self {
        let loaded = Config::load_from(config_path).ok();
        let inside = |p: &Path| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                workdir.join(p)
            }
        };
        let existing = |p: PathBuf| p.exists().then_some(p);
        Self {
            exe: exe.to_path_buf(),
            workdir: workdir.to_path_buf(),
            config: config_path.to_path_buf(),
            env_file: existing(workdir.join(".env")),
            models_dir: existing(workdir.join("models")),
            static_dir: loaded
                .as_ref()
                .and_then(|c| c.server.static_dir.as_deref())
                .map(inside)
                .and_then(existing),
            key_file: loaded
                .as_ref()
                .and_then(|c| c.bridge.key_file.as_deref())
                .map(inside),
        }
    }
}

/// The `icacls` commands that give `account` what the service needs and nothing more:
/// read on the program, the configuration, `.env`, the models, the web UI and the bridge key;
/// modify on `data\` and `logs\` (the audit database and the log files). Windows lets a
/// process open a file by its full path without rights on the folders above it, so the key
/// file in the MT5 folder needs a grant on the file only.
pub fn grant_commands(account: &str, plan: &AccessPlan) -> Vec<String> {
    let grant = |path: &Path, rights: &str, recurse: bool| {
        format!(
            "icacls \"{}\" /grant \"{account}:{rights}\"{}",
            path.display(),
            if recurse { " /T" } else { "" }
        )
    };
    let mut commands = vec![
        // The folder itself: enter it and list it, nothing below it.
        grant(&plan.workdir, "(RX)", false),
        grant(&plan.exe, "(RX)", false),
        grant(&plan.config, "(R)", false),
    ];
    if let Some(env) = &plan.env_file {
        commands.push(grant(env, "(R)", false));
    }
    if let Some(models) = &plan.models_dir {
        // Piper is an executable that lives there.
        commands.push(grant(models, "(OI)(CI)(RX)", true));
    }
    if let Some(web) = &plan.static_dir {
        commands.push(grant(web, "(OI)(CI)(R)", true));
    }
    if let Some(key) = &plan.key_file {
        commands.push(grant(key, "(R)", false));
    }
    for writable in ["data", "logs"] {
        commands.push(grant(&plan.workdir.join(writable), "(OI)(CI)(M)", true));
    }
    commands
}

/// Read a password from the first line of stdin (`--password-stdin`).
fn read_password_line() -> anyhow::Result<String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading the password from stdin")?;
    let line = line.trim_end_matches(['\r', '\n']).to_owned();
    if line.is_empty() {
        return Err(anyhow!("the first line of stdin is empty"));
    }
    Ok(line)
}

/// Name used with `sc.exe` and `net start`.
pub const SERVICE_NAME: &str = "JarvisMT";
const DISPLAY_NAME: &str = "Jarvis MT";
const DESCRIPTION: &str = "Jarvis MT: voice and chat trading assistant for MetaTrader 5";
/// Directory (inside the working directory) of the service's rolling log files.
pub const LOG_DIR: &str = "logs";
const LOG_FILE_PREFIX: &str = "jarvis-api.log";

/// The arguments the SCM passes to the executable.
pub fn launch_arguments(workdir: &Path, config: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--config"),
        config.as_os_str().to_owned(),
        OsString::from("service"),
        OsString::from("run"),
        OsString::from("--workdir"),
        workdir.as_os_str().to_owned(),
    ]
}

/// Restart after a failure: after 5 s, then 15 s, then every 60 s; the failure count resets
/// after a day without failures. Also applies when the process exits with an error code.
pub fn failure_actions() -> ServiceFailureActions {
    let restart = |secs| ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(secs),
    };
    ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart(5), restart(15), restart(60)]),
    }
}

fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))
}

/// Run a `service` subcommand. All but `run` need an elevated prompt.
pub fn execute(command: ServiceCommand, config: Option<PathBuf>) -> anyhow::Result<()> {
    match command {
        ServiceCommand::Install {
            account,
            password_stdin,
            local_system,
        } => {
            let password = if password_stdin {
                Some(read_password_line()?)
            } else {
                None
            };
            install(config, RunAs::from_flags(account, password, local_system)?)
        }
        ServiceCommand::Uninstall => uninstall(),
        ServiceCommand::Start => start(),
        ServiceCommand::Stop => stop(),
        ServiceCommand::Run { workdir } => run(config, workdir),
    }
}

fn manager(access: ServiceManagerAccess) -> anyhow::Result<ServiceManager> {
    ServiceManager::local_computer(None::<&str>, access).context(
        "cannot reach the Service Control Manager (run this from an elevated, administrator prompt)",
    )
}

fn install(config: Option<PathBuf>, run_as: RunAs) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("finding this executable")?;
    let workdir = std::env::current_dir().context("finding the working directory")?;
    let config = absolute(&config.unwrap_or_else(Config::path))?;
    if !config.is_file() {
        return Err(anyhow!(
            "config file {} does not exist; create it first (copy config/jarvis.example.toml)",
            config.display()
        ));
    }
    let info = service_info(exe.clone(), &workdir, &config, &run_as);
    let manager = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    let service = manager
        .create_service(
            &info,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
        )
        .context("creating the service (is it already installed?)")?;
    service.set_description(DESCRIPTION)?;
    service.set_delayed_auto_start(true)?;
    service.update_failure_actions(failure_actions())?;
    service.set_failure_actions_on_non_crash_failures(true)?;
    println!("Service '{SERVICE_NAME}' installed (automatic, delayed start).");
    println!("Working directory: {}", workdir.display());
    println!("Config: {}", config.display());
    match run_as.grantee() {
        None => println!(
            "WARNING: the service runs as LocalSystem, the most privileged account on this \
             machine. A flaw in the server, the audio decoder or the speech engine would then \
             run with full rights. Reinstall without --local-system to use the virtual account \
             {} (see docs/windows-service.md).",
            virtual_account()
        ),
        Some(account) => {
            println!("The service runs as {account}.");
            // The grants below need the folders to exist.
            for dir in ["data", "logs"] {
                std::fs::create_dir_all(workdir.join(dir))
                    .with_context(|| format!("creating {dir}\\"))?;
            }
            let plan = AccessPlan::discover(&exe, &workdir, &config);
            println!(
                "That account has no access to your files yet. Run these commands in this \
                 elevated prompt, then start the service:\n"
            );
            for command in grant_commands(&account, &plan) {
                println!("  {command}");
            }
            if plan.key_file.is_none() {
                println!(
                    "\nNo [bridge] key_file was found in the config. If the bridge key comes \
                     from JARVIS_BRIDGE_KEY in .env, nothing more is needed; otherwise grant \
                     read on the key file the same way:\n  icacls \"<bridge.key>\" /grant \
                     \"{account}:(R)\""
                );
            }
        }
    }
    println!("\nStart it with: jarvis-api service start");
    Ok(())
}

fn open(access: ServiceAccess) -> anyhow::Result<windows_service::service::Service> {
    manager(ServiceManagerAccess::CONNECT)?
        .open_service(SERVICE_NAME, access)
        .with_context(|| format!("opening service '{SERVICE_NAME}' (is it installed?)"))
}

fn uninstall() -> anyhow::Result<()> {
    let service = open(ServiceAccess::DELETE | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        let _ = service.stop();
    }
    service.delete()?;
    println!("Service '{SERVICE_NAME}' removed (it disappears once it has stopped).");
    Ok(())
}

fn start() -> anyhow::Result<()> {
    open(ServiceAccess::START)?.start::<&str>(&[])?;
    println!("Service '{SERVICE_NAME}' is starting. Check logs/ for the result.");
    Ok(())
}

fn stop() -> anyhow::Result<()> {
    open(ServiceAccess::STOP)?.stop()?;
    println!("Service '{SERVICE_NAME}' is stopping.");
    Ok(())
}

// ---------------------------------------------------------------------------
// The SCM entry point
// ---------------------------------------------------------------------------

static RUN_ARGS: OnceLock<(Option<PathBuf>, Option<PathBuf>)> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

/// `service run`: hand control to the dispatcher; blocks until the service stops.
fn run(config: Option<PathBuf>, workdir: Option<PathBuf>) -> anyhow::Result<()> {
    let _ = RUN_ARGS.set((config, workdir));
    service_dispatcher::start(SERVICE_NAME, ffi_service_main).context(
        "starting the service dispatcher (`service run` is started by the Service Control \
         Manager; to run in a console just use `jarvis-api`)",
    )
}

fn service_main(_arguments: Vec<OsString>) {
    // `run_service` reports the final state to the SCM itself; a failure before it could
    // register with the SCM is only visible in the log (or the event log via the SCM).
    if let Err(e) = run_service() {
        tracing::error!("service failed: {e:#}");
    }
}

fn status(
    state: ServiceState,
    exit_code: ServiceExitCode,
    accept: ServiceControlAccept,
) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::from_secs(
            if matches!(state, ServiceState::Running | ServiceState::Stopped) {
                0
            } else {
                30
            },
        ),
        process_id: None,
    }
}

fn run_service() -> anyhow::Result<()> {
    let (config, workdir) = RUN_ARGS.get().cloned().unwrap_or_default();
    if let Some(dir) = &workdir {
        std::env::set_current_dir(dir)
            .with_context(|| format!("entering the working directory {}", dir.display()))?;
    }
    let _log_guard = init_file_logging();

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let stop_tx = Mutex::new(Some(stop_tx));
    let handle = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            if let Some(tx) = stop_tx.lock().unwrap_or_else(|p| p.into_inner()).take() {
                let _ = tx.send(());
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceExitCode::NO_ERROR,
        ServiceControlAccept::empty(),
    ))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    handle.set_service_status(status(
        ServiceState::Running,
        ServiceExitCode::NO_ERROR,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
    ))?;
    let result = runtime.block_on(crate::serve::run(config, async {
        let _ = stop_rx.await;
    }));
    let exit = match &result {
        Ok(()) => ServiceExitCode::NO_ERROR,
        Err(e) => {
            tracing::error!("jarvis-api stopped with an error: {e:#}");
            ServiceExitCode::ServiceSpecific(1)
        }
    };
    handle.set_service_status(status(
        ServiceState::Stopped,
        exit,
        ServiceControlAccept::empty(),
    ))?;
    result
}

/// Daily rolling file in `logs/`, no colour codes. The returned guard flushes on drop.
fn init_file_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::EnvFilter;
    let _ = dotenvy::dotenv();
    let appender = tracing_appender::rolling::daily(LOG_DIR, LOG_FILE_PREFIX);
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,jarvis_api=debug,jarvis_bridge_client=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init()
        .ok()?;
    Some(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_arguments_carry_config_and_workdir() {
        let args = launch_arguments(
            Path::new("D:/Jarvis"),
            Path::new("D:/Jarvis/config/jarvis.toml"),
        );
        let text: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            text,
            [
                "--config",
                "D:/Jarvis/config/jarvis.toml",
                "service",
                "run",
                "--workdir",
                "D:/Jarvis"
            ]
        );
        // The service entry point parses what install registers.
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(
            std::iter::once(OsString::from("jarvis-api")).chain(args),
        )
        .unwrap();
        assert_eq!(
            cli.config,
            Some(PathBuf::from("D:/Jarvis/config/jarvis.toml"))
        );
        assert_eq!(
            cli.command,
            Some(crate::cli::Command::Service {
                command: ServiceCommand::Run {
                    workdir: Some(PathBuf::from("D:/Jarvis"))
                }
            })
        );
    }

    #[test]
    fn the_service_runs_as_its_virtual_account_by_default() {
        let run_as = RunAs::from_flags(None, None, false).unwrap();
        assert_eq!(run_as, RunAs::VirtualAccount);
        let info = service_info(
            PathBuf::from(r"D:\Jarvis\jarvis-api.exe"),
            Path::new(r"D:\Jarvis"),
            Path::new(r"D:\Jarvis\config\jarvis.toml"),
            &run_as,
        );
        assert_eq!(
            info.account_name,
            Some(OsString::from(r"NT SERVICE\JarvisMT"))
        );
        assert_eq!(
            info.account_password, None,
            "a virtual account has no password"
        );
        assert_eq!(info.name, OsString::from("JarvisMT"));
        assert_eq!(info.start_type, ServiceStartType::AutoStart);
        assert_eq!(info.service_type, ServiceType::OWN_PROCESS);
        assert_eq!(
            info.launch_arguments,
            launch_arguments(
                Path::new(r"D:\Jarvis"),
                Path::new(r"D:\Jarvis\config\jarvis.toml")
            )
        );
        // The virtual account is named after the service.
        assert_eq!(virtual_account(), format!("NT SERVICE\\{SERVICE_NAME}"));
    }

    #[test]
    fn local_system_and_other_accounts_are_explicit_choices() {
        let local = RunAs::from_flags(None, None, true).unwrap();
        assert_eq!(local, RunAs::LocalSystem);
        assert_eq!(local.account_name(), None);
        assert_eq!(local.account_password(), None);
        assert_eq!(local.grantee(), None, "LocalSystem needs no grants");

        let service_account =
            RunAs::from_flags(Some(r"NT AUTHORITY\LocalService".into()), None, false).unwrap();
        assert_eq!(
            service_account.account_name(),
            Some(OsString::from(r"NT AUTHORITY\LocalService"))
        );
        assert_eq!(service_account.account_password(), None);

        let with_password =
            RunAs::from_flags(Some(r".\jarvis-svc".into()), Some("s3cret".into()), false).unwrap();
        assert_eq!(
            with_password.account_password(),
            Some(OsString::from("s3cret"))
        );
        assert_eq!(with_password.grantee().as_deref(), Some(r".\jarvis-svc"));
        // The password never shows up in debug output of the plan the user sees.
        assert!(RunAs::from_flags(Some("  ".into()), None, false).is_err());
        assert!(RunAs::from_flags(Some("x".into()), None, true).is_err());
        assert!(RunAs::from_flags(None, Some("p".into()), false).is_err());
    }

    fn plan() -> AccessPlan {
        AccessPlan {
            exe: PathBuf::from(r"D:\Jarvis\bin\jarvis-api.exe"),
            workdir: PathBuf::from(r"D:\Jarvis"),
            config: PathBuf::from(r"D:\Jarvis\config\jarvis.toml"),
            env_file: Some(PathBuf::from(r"D:\Jarvis\.env")),
            models_dir: Some(PathBuf::from(r"D:\Jarvis\models")),
            static_dir: Some(PathBuf::from(r"D:\Jarvis\web\build")),
            key_file: Some(PathBuf::from(
                r"C:\Users\me\AppData\Roaming\MetaQuotes\Terminal\ABC\MQL5\Files\JarvisMT\bridge.key",
            )),
        }
    }

    #[test]
    fn the_printed_grants_give_exactly_what_the_service_needs() {
        let commands = grant_commands(r"NT SERVICE\JarvisMT", &plan());
        assert_eq!(
            commands,
            [
                r#"icacls "D:\Jarvis" /grant "NT SERVICE\JarvisMT:(RX)""#,
                r#"icacls "D:\Jarvis\bin\jarvis-api.exe" /grant "NT SERVICE\JarvisMT:(RX)""#,
                r#"icacls "D:\Jarvis\config\jarvis.toml" /grant "NT SERVICE\JarvisMT:(R)""#,
                r#"icacls "D:\Jarvis\.env" /grant "NT SERVICE\JarvisMT:(R)""#,
                r#"icacls "D:\Jarvis\models" /grant "NT SERVICE\JarvisMT:(OI)(CI)(RX)" /T"#,
                r#"icacls "D:\Jarvis\web\build" /grant "NT SERVICE\JarvisMT:(OI)(CI)(R)" /T"#,
                r#"icacls "C:\Users\me\AppData\Roaming\MetaQuotes\Terminal\ABC\MQL5\Files\JarvisMT\bridge.key" /grant "NT SERVICE\JarvisMT:(R)""#,
                r#"icacls "D:\Jarvis\data" /grant "NT SERVICE\JarvisMT:(OI)(CI)(M)" /T"#,
                r#"icacls "D:\Jarvis\logs" /grant "NT SERVICE\JarvisMT:(OI)(CI)(M)" /T"#,
            ]
        );
        // Write access is for data\ and logs\ only.
        let writers: Vec<_> = commands.iter().filter(|c| c.contains("(M)")).collect();
        assert_eq!(writers.len(), 2);
        // Nothing is granted to everybody, and nothing outside the listed paths.
        assert!(commands.iter().all(|c| !c.contains("Everyone")));
        assert!(
            commands
                .iter()
                .all(|c| c.starts_with("icacls \"") && c.contains("/grant"))
        );
    }

    #[test]
    fn paths_that_do_not_exist_get_no_grant() {
        let bare = AccessPlan {
            env_file: None,
            models_dir: None,
            static_dir: None,
            key_file: None,
            ..plan()
        };
        let commands = grant_commands(r".\svc", &bare);
        assert_eq!(commands.len(), 5, "{commands:#?}");
        assert!(commands.iter().all(|c| c.contains(r#".\svc:"#)));
    }

    #[test]
    fn discovery_reads_the_key_file_and_web_folder_from_the_config() {
        let dir = std::env::temp_dir().join(format!("jarvis-svc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("web").join("build")).unwrap();
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join(".env"), "X=1\n").unwrap();
        let key = dir.join("bridge.key");
        std::fs::write(
            dir.join("config").join("jarvis.toml"),
            format!(
                "[server]\nstatic_dir = 'web/build'\n[bridge]\nkey_file = '{}'\n\
                 [llm]\ndefault = 'l'\n[[llm.providers]]\nname = 'l'\nkind = 'ollama'\nmodel = 'm'\n",
                key.display()
            ),
        )
        .unwrap();
        let plan = AccessPlan::discover(
            Path::new("C:/bin/jarvis-api.exe"),
            &dir,
            &dir.join("config").join("jarvis.toml"),
        );
        assert_eq!(plan.env_file, Some(dir.join(".env")));
        assert_eq!(plan.models_dir, Some(dir.join("models")));
        assert_eq!(plan.static_dir, Some(dir.join("web/build")));
        assert_eq!(plan.key_file, Some(key));
        // A config that cannot be read is not an error here: the lists are just shorter.
        let broken = AccessPlan::discover(Path::new("x.exe"), &dir, &dir.join("missing.toml"));
        assert_eq!((broken.static_dir, broken.key_file), (None, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failures_restart_the_service() {
        let f = failure_actions();
        let actions = f.actions.unwrap();
        assert_eq!(actions.len(), 3);
        assert!(
            actions
                .iter()
                .all(|a| a.action_type == ServiceActionType::Restart)
        );
        assert_eq!(actions[0].delay, Duration::from_secs(5));
        assert!(matches!(
            f.reset_period,
            ServiceFailureResetPeriod::After(_)
        ));
    }

    #[test]
    fn service_name_is_stable() {
        assert_eq!(SERVICE_NAME, "JarvisMT");
        assert_eq!(LOG_DIR, "logs");
    }
}
