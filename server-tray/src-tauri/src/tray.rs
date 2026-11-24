//! The tray icon and its menu.

use crate::supervisor::Status;
use crate::{config, firewall, open_in_browser, state, updates};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;

const OPEN: &str = "open";
const COPY_LINK: &str = "copy-link";
const TOGGLE: &str = "toggle";
const RESTART: &str = "restart";
const AUTOSTART: &str = "autostart";
const CHECK_UPDATES: &str = "check-updates";
const UPDATE_NOW: &str = "update-now";
const SETTINGS: &str = "settings";
const LOGS: &str = "logs";
const FIREWALL: &str = "firewall";
const QUIT: &str = "quit";

/// Menu entries whose text or state changes while the app runs.
pub(crate) struct TrayItems {
    status: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    autostart: CheckMenuItem<Wry>,
    update_now: MenuItem<Wry>,
}

pub(crate) fn version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

pub(crate) fn build(app: &AppHandle) -> tauri::Result<()> {
    let version = version(app);
    let header = MenuItem::new(
        app,
        format!("Omnilogue Server v{version}"),
        false,
        None::<&str>,
    )?;
    let status = MenuItem::new(app, "Starting…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, OPEN, "Open in browser", true, None::<&str>)?;
    let copy = MenuItem::with_id(
        app,
        COPY_LINK,
        "Copy access link (includes token)",
        true,
        None::<&str>,
    )?;
    let toggle = MenuItem::with_id(app, TOGGLE, "Stop server", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, RESTART, "Restart server", true, None::<&str>)?;
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        AUTOSTART,
        "Start with Windows",
        true,
        autostart_on,
        None::<&str>,
    )?;
    let check = MenuItem::with_id(
        app,
        CHECK_UPDATES,
        format!("Check for updates (current v{version})"),
        true,
        None::<&str>,
    )?;
    let update_now = MenuItem::with_id(
        app,
        UPDATE_NOW,
        "Update now (no update found yet)",
        false,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(
        app,
        SETTINGS,
        "Edit settings… (then Restart server)",
        true,
        None::<&str>,
    )?;
    let logs = MenuItem::with_id(app, LOGS, "Open logs folder", true, None::<&str>)?;
    let firewall = MenuItem::with_id(
        app,
        FIREWALL,
        "Allow through Windows Firewall…",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, QUIT, "Quit (stops the server)", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &header,
            &status,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &copy,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &restart,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &check,
            &update_now,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &logs,
            &firewall,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Omnilogue Server")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()));
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    app.manage(TrayItems {
        status,
        toggle,
        autostart,
        update_now,
    });
    Ok(())
}

/// Called from the supervisor thread; menu changes happen on the main thread.
/// `run_on_main_thread` only queues the change, so it never blocks the
/// supervisor (which the main thread may be waiting on during shutdown).
pub(crate) fn show_status(app: &AppHandle, status: &Status) {
    let status = status.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(items) = handle.try_state::<TrayItems>() else {
            return;
        };
        let _ = items.status.set_text(status.label());
        let running = !matches!(status, Status::Stopped);
        let _ = items.toggle.set_text(if running {
            "Stop server"
        } else {
            "Start server"
        });
        if let Some(tray) = handle.tray_by_id("main") {
            let _ = tray.set_tooltip(Some(format!("Omnilogue Server: {}", status.label())));
        }
    });
}

pub(crate) fn refresh_autostart(app: &AppHandle) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items
            .autostart
            .set_checked(app.autolaunch().is_enabled().unwrap_or(false));
    }
}

pub(crate) fn show_update(app: &AppHandle, new_version: Option<&str>) {
    let text = new_version.map_or_else(
        || "Update now (no update found yet)".to_string(),
        |v| format!("Update now to v{v}"),
    );
    let enabled = new_version.is_some();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(items) = handle.try_state::<TrayItems>() {
            let _ = items.update_now.set_text(text);
            let _ = items.update_now.set_enabled(enabled);
        }
    });
}

fn handle_menu(app: &AppHandle, id: &str) {
    log::info!("menu: {id}");
    let state = state(app);
    match id {
        OPEN => open_in_browser(app),
        COPY_LINK => copy_link(app),
        TOGGLE => {
            if state.supervisor.is_enabled() {
                state.supervisor.stop();
            } else {
                state.supervisor.start();
            }
        }
        RESTART => state.supervisor.restart(),
        AUTOSTART => {
            let launcher = app.autolaunch();
            let result = if launcher.is_enabled().unwrap_or(false) {
                launcher.disable()
            } else {
                launcher.enable()
            };
            if let Err(e) = result {
                log::warn!("cannot change start with Windows: {e}");
            }
            refresh_autostart(app);
        }
        CHECK_UPDATES => in_background(app, |app| updates::check(&app, true)),
        UPDATE_NOW => in_background(app, |app| updates::install_pending(&app)),
        SETTINGS => edit_settings(app),
        LOGS => open_logs(app),
        FIREWALL => in_background(app, |app| firewall::offer(&app, true)),
        QUIT => app.exit(0),
        _ => {}
    }
}

/// Dialogs block, and menu events arrive on the main thread.
fn in_background(app: &AppHandle, work: impl FnOnce(AppHandle) + Send + 'static) {
    let app = app.clone();
    std::thread::spawn(move || work(app));
}

fn copy_link(app: &AppHandle) {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    match config::load_or_create(&state(app).config_dir) {
        Ok((config, _)) => {
            if let Err(e) = app.clipboard().write_text(config.access_url()) {
                log::warn!("cannot copy to the clipboard: {e}");
            }
        }
        Err(e) => log::warn!("cannot read settings: {e}"),
    }
}

fn edit_settings(app: &AppHandle) {
    let dir = &state(app).config_dir;
    if let Err(e) = config::load_or_create(dir) {
        log::warn!("settings file is invalid, opening it anyway: {e}");
    }
    let path = config::path_in(dir);
    // Through the shell, not CreateProcess: on Windows 11 notepad.exe may be
    // only an app execution alias for the Store version.
    if let Err(e) = app
        .opener()
        .open_path(path.to_string_lossy(), Some("notepad.exe"))
    {
        show_error(
            app,
            &format!("Cannot open {} in Notepad: {e}", path.display()),
        );
    }
}

/// Opens Explorer on the logs folder, with server.log selected.
fn open_logs(app: &AppHandle) {
    let dir = &state(app).log_dir;
    let _ = std::fs::create_dir_all(dir);
    let file = dir.join("server.log");
    let result = if file.exists() {
        app.opener().reveal_item_in_dir(&file)
    } else {
        app.opener().open_path(dir.to_string_lossy(), None::<&str>)
    };
    if let Err(e) = result {
        show_error(app, &format!("Cannot open {}: {e}", dir.display()));
    }
}

/// Logs the error and shows it, without blocking the menu.
fn show_error(app: &AppHandle, text: &str) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    log::warn!("{text}");
    app.dialog()
        .message(text)
        .title("Omnilogue Server")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}
