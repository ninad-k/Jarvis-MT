//! Windows service: `jarvis-api service install|uninstall|start|stop|run`.
//!
//! `install` registers this executable with the arguments
//! `--config <abs> service run --workdir <abs>`, so the service finds `config/`, `data/`,
//! `models/` and `.env` (loaded from the working directory) the same way a console run does.
//! It starts automatically after boot (delayed) and restarts itself after a failure.
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
        ServiceCommand::Install => install(config),
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

fn install(config: Option<PathBuf>) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("finding this executable")?;
    let workdir = std::env::current_dir().context("finding the working directory")?;
    let config = absolute(&config.unwrap_or_else(Config::path))?;
    if !config.is_file() {
        return Err(anyhow!(
            "config file {} does not exist; create it first (copy config/jarvis.example.toml)",
            config.display()
        ));
    }
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: launch_arguments(&workdir, &config),
        dependencies: vec![],
        // LocalSystem. To run as a dedicated account instead: `sc.exe config JarvisMT obj= ...`
        account_name: None,
        account_password: None,
    };
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
    println!(
        "Note: the service runs as LocalSystem, the most privileged account. For least privilege \
         run it as a dedicated account that can read this directory and write data/ and logs/ \
         (see docs/windows-service.md and docs/security/threat-model.md)."
    );
    println!("Start it with: jarvis-api service start");
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
