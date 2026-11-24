//! Omnilogue Server tray app: runs the Omnilogue web UI server in the
//! background on Windows, with a tray menu to control it, start it with
//! Windows, and update it.

mod config;
mod firewall;
mod supervisor;
mod tray;
mod updates;

use std::path::PathBuf;
use std::sync::Mutex;
use supervisor::Supervisor;
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};

/// Passed by the "Start with Windows" entry, so a login start stays quiet.
const AUTOSTART_ARG: &str = "--autostart";

pub(crate) struct AppState {
    pub supervisor: Supervisor,
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
    pub server_exe: PathBuf,
    pub pending_update: Mutex<Option<tauri_plugin_updater::Update>>,
}

pub(crate) fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

/// Opens the web UI in the default browser, starting the server if needed.
pub(crate) fn open_in_browser(app: &AppHandle) {
    use tauri_plugin_opener::OpenerExt;
    let state = state(app);
    if !state.supervisor.is_enabled() {
        state.supervisor.start();
    }
    match config::load_or_create(&state.config_dir) {
        Ok((config, _)) => {
            if let Err(e) = app.opener().open_url(config.local_url(), None::<&str>) {
                log::warn!("cannot open the browser: {e}");
            }
        }
        Err(e) => log::warn!("cannot read settings: {e}"),
    }
}

/// # Panics
/// Panics if the Tauri runtime cannot start.
pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch (e.g. from the Start menu) opens the
        // browser instead of starting another server.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_in_browser(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            setup(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while starting Omnilogue Server")
        .run(|app, event| match event {
            // There are no windows; only "Quit" (which passes a code) exits.
            RunEvent::ExitRequested {
                api, code: None, ..
            } => api.prevent_exit(),
            RunEvent::Exit => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.supervisor.shutdown();
                }
            }
            _ => {}
        });
}

fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = app.path().app_config_dir()?;
    let log_dir = app.path().app_log_dir()?;
    let server_exe = std::env::current_exe()?
        .parent()
        .ok_or("the app has no install directory")?
        .join(if cfg!(windows) {
            "cchv-server.exe"
        } else {
            "cchv-server"
        });

    let (config, first_run) = config::load_or_create(&config_dir)?;

    tray::build(app)?;

    let handle = app.clone();
    let supervisor = Supervisor::new(
        server_exe.clone(),
        config_dir.clone(),
        &log_dir,
        move |status| {
            tray::show_status(&handle, status);
        },
    );
    app.manage(AppState {
        supervisor,
        config_dir,
        log_dir,
        server_exe,
        pending_update: Mutex::new(None),
    });
    state(app).supervisor.start();

    let launched_at_login = std::env::args().any(|a| a == AUTOSTART_ARG);
    if first_run {
        if let Err(e) = app.autolaunch().enable() {
            log::warn!("cannot enable start with Windows: {e}");
        }
        tray::refresh_autostart(app);
        let handle = app.clone();
        let exposed = config.is_network_exposed();
        std::thread::spawn(move || {
            if exposed {
                firewall::offer(&handle, false);
            }
            open_in_browser(&handle);
        });
    } else if !launched_at_login {
        open_in_browser(app);
    }

    updates::start_background_checks(app);
    Ok(())
}
