//! Self-update through the Tauri updater: a signed NSIS installer and a
//! `latest-server-tray.json` manifest on this project's GitHub releases.

use crate::{config, state, tray};
use std::time::Duration;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Checks shortly after startup and then daily, if enabled in the settings.
/// Each new version is announced with a dialog once per run.
pub(crate) fn start_background_checks(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK_DELAY);
        loop {
            let enabled = config::load_or_create(&state(&app).config_dir)
                .map_or(true, |(c, _)| c.check_for_updates);
            if enabled {
                check(&app, false);
            }
            std::thread::sleep(CHECK_INTERVAL);
        }
    });
}

/// Looks for a newer version. `interactive` is true when the user asked, so
/// "no update" and errors are reported too. Blocks; call off the main thread.
pub(crate) fn check(app: &AppHandle, interactive: bool) {
    let current = tray::version(app);
    let result = tauri::async_runtime::block_on(async { app.updater()?.check().await });
    match result {
        Ok(Some(update)) => {
            let new_version = update.version.clone();
            let already_announced = state(app)
                .pending_update
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replace(update)
                .is_some_and(|old| old.version == new_version);
            tray::show_update(app, Some(&new_version));
            if interactive || !already_announced {
                let install = app
                    .dialog()
                    .message(format!(
                        "Version {new_version} is available. You have {current}.\n\n\
                         Installing stops the server for a moment and restarts it."
                    ))
                    .title("Omnilogue Server update")
                    .kind(MessageDialogKind::Info)
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "Install now".into(),
                        "Later".into(),
                    ))
                    .blocking_show();
                if install {
                    install_pending(app);
                }
            }
        }
        Ok(None) => {
            tray::show_update(app, None);
            if interactive {
                message(
                    app,
                    MessageDialogKind::Info,
                    &format!("You have the latest version ({current})."),
                );
            }
        }
        Err(e) => {
            log::warn!("update check failed: {e}");
            if interactive {
                message(
                    app,
                    MessageDialogKind::Error,
                    &format!("Could not check for updates:\n{e}"),
                );
            }
        }
    }
}

/// Downloads and installs the update found by the last check. The server is
/// stopped only after the download succeeds, because the installer has to
/// replace its exe. The installer restarts the app when it finishes.
pub(crate) fn install_pending(app: &AppHandle) {
    let update = state(app)
        .pending_update
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(update) = update else {
        check(app, true);
        return;
    };
    let bytes = match tauri::async_runtime::block_on(update.download(|_, _| {}, || {})) {
        Ok(bytes) => bytes,
        Err(e) => {
            message(
                app,
                MessageDialogKind::Error,
                &format!("Could not download the update:\n{e}"),
            );
            return;
        }
    };
    state(app).supervisor.shutdown();
    // On Windows this starts the installer and exits the app.
    if let Err(e) = update.install(bytes) {
        message(
            app,
            MessageDialogKind::Error,
            &format!("Could not install the update:\n{e}\n\nOmnilogue Server will restart."),
        );
        app.restart();
    }
}

fn message(app: &AppHandle, kind: MessageDialogKind, text: &str) {
    app.dialog()
        .message(text)
        .title("Omnilogue Server")
        .kind(kind)
        .blocking_show();
}
