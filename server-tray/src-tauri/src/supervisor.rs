//! Runs the bundled `cchv-server.exe` (the upstream server binary, built with
//! `--features webui-server`) as a child process, restarts it when it exits,
//! and reports its status to the tray.
//!
//! The server runs as a separate process rather than in-process because the
//! upstream entry point (`run_server`) parses its own arguments, calls
//! `process::exit` on startup errors and stops only on Ctrl+C. See UPSTREAM.md
//! for the small refactor that would let the tray link it directly.

use crate::config::{self, ServerConfig};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const MAX_BACKOFF_SECS: u64 = 60;
/// A run longer than this counts as healthy and resets the restart backoff.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Running {
        address: String,
    },
    /// The server exited on its own and restarts after a delay.
    Restarting {
        exit_code: Option<i32>,
        retry_in_secs: u64,
    },
    /// It could not be started (missing exe, unreadable settings).
    Failed(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Stopped => "Stopped".into(),
            Status::Running { address } => format!("Running at {address}"),
            Status::Restarting {
                exit_code: Some(code),
                retry_in_secs,
            } => {
                format!("Exited (code {code}), restarting in {retry_in_secs}s")
            }
            Status::Restarting {
                exit_code: None,
                retry_in_secs,
            } => {
                format!("Exited, restarting in {retry_in_secs}s")
            }
            Status::Failed(reason) => format!("Not running: {reason}"),
        }
    }
}

type Listener = Box<dyn Fn(&Status) + Send + Sync>;

#[derive(Default)]
struct Control {
    want_running: bool,
    restart: bool,
    shutdown: bool,
}

struct Shared {
    control: Mutex<Control>,
    wake: Condvar,
    status: Mutex<Status>,
    server_exe: PathBuf,
    config_dir: PathBuf,
    log_path: PathBuf,
    listener: Listener,
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

pub struct Supervisor {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Supervisor {
    /// Creates the supervisor in the stopped state; call [`Supervisor::start`].
    pub fn new(
        server_exe: PathBuf,
        config_dir: PathBuf,
        log_dir: &Path,
        listener: impl Fn(&Status) + Send + Sync + 'static,
    ) -> Self {
        let _ = fs::create_dir_all(log_dir);
        let shared = Arc::new(Shared {
            control: Mutex::new(Control::default()),
            wake: Condvar::new(),
            status: Mutex::new(Status::Stopped),
            server_exe,
            config_dir,
            log_path: log_dir.join("server.log"),
            listener: Box::new(listener),
            #[cfg(windows)]
            job: kill_on_close_job(),
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("server-supervisor".into())
            .spawn(move || supervise(&worker))
            .expect("failed to start the supervisor thread");
        Self {
            shared,
            thread: Mutex::new(Some(thread)),
        }
    }

    /// Whether the user wants the server running (it may be between restarts).
    pub fn is_enabled(&self) -> bool {
        lock(&self.shared.control).want_running
    }

    pub fn start(&self) {
        self.update(|c| c.want_running = true);
    }

    pub fn stop(&self) {
        self.update(|c| c.want_running = false);
    }

    /// Restarts now, re-reading the settings file. Starts it if stopped.
    pub fn restart(&self) {
        self.update(|c| {
            c.want_running = true;
            c.restart = true;
        });
    }

    /// Stops the server and ends the supervisor thread. Blocks until the
    /// server process is gone, so the installer can replace its exe.
    pub fn shutdown(&self) {
        self.update(|c| c.shutdown = true);
        if let Some(thread) = lock(&self.thread).take() {
            let _ = thread.join();
        }
    }

    fn update(&self, change: impl FnOnce(&mut Control)) {
        change(&mut lock(&self.shared.control));
        self.shared.wake.notify_all();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn set_status(shared: &Shared, status: Status) {
    *lock(&shared.status) = status.clone();
    (shared.listener)(&status);
}

enum Ended {
    /// Stopped, restarted or shut down from the tray.
    Requested,
    /// Exited by itself.
    Exited(Option<i32>),
}

fn supervise(shared: &Shared) {
    let mut consecutive_failures: u32 = 0;
    loop {
        {
            let mut control = lock(&shared.control);
            while !control.want_running && !control.shutdown {
                control = shared
                    .wake
                    .wait(control)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if control.shutdown {
                return;
            }
            control.restart = false;
        }

        let config = match config::load_or_create(&shared.config_dir) {
            Ok((config, _)) => config,
            Err(e) => {
                log_line(shared, &format!("cannot read settings: {e}"));
                set_status(
                    shared,
                    Status::Failed(format!("settings file is invalid ({e})")),
                );
                wait_for_change(shared, Duration::from_secs(30));
                continue;
            }
        };

        let mut child = match spawn(shared, &config) {
            Ok(child) => child,
            Err(e) => {
                log_line(
                    shared,
                    &format!("cannot start {}: {e}", shared.server_exe.display()),
                );
                set_status(
                    shared,
                    Status::Failed(format!("cannot start the server ({e})")),
                );
                wait_for_change(shared, Duration::from_secs(30));
                continue;
            }
        };
        let started = Instant::now();
        set_status(
            shared,
            Status::Running {
                address: config.display_address(),
            },
        );

        match wait_for_exit(shared, &mut child) {
            Ended::Requested => {
                let _ = child.kill();
                let _ = child.wait();
                log_line(shared, "server stopped");
                consecutive_failures = 0;
                let control = lock(&shared.control);
                if control.shutdown {
                    drop(control);
                    set_status(shared, Status::Stopped);
                    return;
                }
                if !control.want_running {
                    drop(control);
                    set_status(shared, Status::Stopped);
                }
            }
            Ended::Exited(code) => {
                consecutive_failures = if started.elapsed() >= HEALTHY_RUN {
                    1
                } else {
                    consecutive_failures.saturating_add(1)
                };
                let delay = backoff_secs(consecutive_failures);
                log_line(
                    shared,
                    &format!("server exited with code {code:?}; restarting in {delay}s (see the lines above)"),
                );
                set_status(
                    shared,
                    Status::Restarting {
                        exit_code: code,
                        retry_in_secs: delay,
                    },
                );
                wait_for_change(shared, Duration::from_secs(delay));
                if !lock(&shared.control).want_running {
                    set_status(shared, Status::Stopped);
                }
            }
        }
    }
}

/// 2, 4, 8 … seconds, capped at a minute.
fn backoff_secs(consecutive_failures: u32) -> u64 {
    2u64.saturating_pow(consecutive_failures.clamp(1, 6))
        .min(MAX_BACKOFF_SECS)
}

fn wait_for_exit(shared: &Shared, child: &mut Child) -> Ended {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ended::Exited(status.code()),
            Ok(None) => {}
            Err(_) => return Ended::Exited(None),
        }
        let control = lock(&shared.control);
        if control.shutdown || !control.want_running || control.restart {
            return Ended::Requested;
        }
        let _ = shared.wake.wait_timeout(control, POLL);
    }
}

/// Sleeps up to `timeout`, waking early when the user stops, restarts or quits.
fn wait_for_change(shared: &Shared, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut control = lock(&shared.control);
    while control.want_running && !control.restart && !control.shutdown {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        control = shared
            .wake
            .wait_timeout(control, deadline - now)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
    }
}

fn spawn(shared: &Shared, config: &ServerConfig) -> std::io::Result<Child> {
    if !shared.server_exe.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "{} is missing; reinstall Omnilogue Server",
                shared.server_exe.display()
            ),
        ));
    }
    let log = open_log(&shared.log_path)?;
    log_line(
        shared,
        &format!("starting server on {}:{}", config.host, config.port),
    );

    let mut command = Command::new(&shared.server_exe);
    command
        .args([
            "--serve",
            "--host",
            &config.host,
            "--port",
            &config.port.to_string(),
        ])
        .args(&config.extra_args)
        // The token goes in the environment so it is not visible in the
        // process list (the server reads CCHV_TOKEN when --token is absent).
        .env("CCHV_TOKEN", &config.token)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command.spawn()?;

    #[cfg(windows)]
    if let Some(job) = &shared.job {
        use std::os::windows::io::AsRawHandle;
        if let Err(e) = job.assign_process(child.as_raw_handle() as isize) {
            log_line(
                shared,
                &format!("warning: could not tie the server to the tray's lifetime: {e:?}"),
            );
        }
    }
    Ok(child)
}

fn open_log(path: &Path) -> std::io::Result<File> {
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = fs::rename(path, path.with_extension("log.old"));
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn log_line(shared: &Shared, message: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&shared.log_path)
    {
        let _ = writeln!(
            file,
            "[{}] tray: {message}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }
}

#[cfg(windows)]
fn kill_on_close_job() -> Option<win32job::Job> {
    let job = win32job::Job::create().ok()?;
    let mut info = job.query_extended_limit_info().ok()?;
    info.limit_kill_on_job_close();
    job.set_extended_limit_info(&info).ok()?;
    Some(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(0), 2);
        assert_eq!(backoff_secs(1), 2);
        assert_eq!(backoff_secs(2), 4);
        assert_eq!(backoff_secs(5), 32);
        assert_eq!(backoff_secs(6), 60);
        assert_eq!(backoff_secs(50), 60);
    }

    #[test]
    fn status_labels_read_naturally() {
        assert_eq!(
            Status::Running {
                address: "http://x:1".into()
            }
            .label(),
            "Running at http://x:1"
        );
        assert_eq!(
            Status::Restarting {
                exit_code: Some(1),
                retry_in_secs: 4
            }
            .label(),
            "Exited (code 1), restarting in 4s"
        );
    }
}
