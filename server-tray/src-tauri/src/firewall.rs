//! Adds an inbound Windows Firewall rule for the server, after asking.
//!
//! The rule is tied to the server exe, the configured port and the addresses
//! in `firewallRemoteAddress` (the local subnet by default). Adding it needs
//! administrator rights, so Windows shows a UAC prompt.

use crate::{config, state};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

pub const RULE_NAME: &str = "Omnilogue Server";

/// Asks, then adds or replaces the rule. `explicit` is true when the user
/// chose the menu entry; on first run the question explains why it is asked.
/// Blocks; call off the main thread.
pub(crate) fn offer(app: &AppHandle, explicit: bool) {
    let state = state(app);
    let config = match config::load_or_create(&state.config_dir) {
        Ok((config, _)) => config,
        Err(e) => {
            message(
                app,
                MessageDialogKind::Error,
                &format!("Cannot read settings:\n{e}"),
            );
            return;
        }
    };
    let remote = config.firewall_remote_address.trim();
    if !is_valid_remote_address(remote) {
        message(
            app,
            MessageDialogKind::Error,
            &format!(
                "firewallRemoteAddress in server.json is not valid: \"{remote}\".\n\
                 Use LocalSubnet, or addresses like 192.168.1.10 or 192.168.1.0/24, comma-separated."
            ),
        );
        return;
    }
    let intro = if explicit {
        String::new()
    } else {
        "The server listens on your network so phones and other devices can reach it. \
         Windows Firewall blocks that until you allow it.\n\n"
            .to_string()
    };
    let allow = app
        .dialog()
        .message(format!(
            "{intro}Allow incoming connections to Omnilogue Server on TCP port {} from {remote}?\n\n\
             Windows will ask for administrator permission. You can change who is allowed with \
             firewallRemoteAddress in the settings.",
            config.port
        ))
        .title("Allow through Windows Firewall")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom("Allow".into(), "Not now".into()))
        .blocking_show();
    if !allow {
        return;
    }

    let script = rule_script(&state.server_exe.to_string_lossy(), config.port, remote);
    match run_elevated(&script) {
        Ok(true) => message(app, MessageDialogKind::Info, "The firewall rule was added."),
        Ok(false) => message(
            app,
            MessageDialogKind::Warning,
            "The firewall rule was not added (permission was declined or the command failed).",
        ),
        Err(e) => message(
            app,
            MessageDialogKind::Error,
            &format!("Cannot run PowerShell:\n{e}"),
        ),
    }
}

/// `LocalSubnet`, or IPv4/IPv6 addresses, ranges and prefixes, comma-separated.
fn is_valid_remote_address(value: &str) -> bool {
    !value.is_empty()
        && value.split(',').all(|part| {
            let part = part.trim();
            !part.is_empty()
                && (part.eq_ignore_ascii_case("LocalSubnet")
                    || part
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '/' | '-')))
        })
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn rule_script(program: &str, port: u16, remote: &str) -> String {
    let remote_list = remote
        .split(',')
        .map(|p| ps_quote(p.trim()))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         Get-NetFirewallRule -DisplayName {name} -ErrorAction SilentlyContinue | Remove-NetFirewallRule\n\
         New-NetFirewallRule -DisplayName {name} -Direction Inbound -Action Allow -Protocol TCP \
         -LocalPort {port} -Program {program} -RemoteAddress {remote_list} -Profile Any | Out-Null\n",
        name = ps_quote(RULE_NAME),
        program = ps_quote(program),
    )
}

/// PowerShell's `-EncodedCommand` takes base64 of UTF-16LE text, which avoids
/// every quoting problem between here and the elevated process.
fn encode_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    STANDARD.encode(bytes)
}

/// Runs the script in an elevated PowerShell. Returns whether it succeeded.
fn run_elevated(script: &str) -> std::io::Result<bool> {
    let launcher = format!(
        "$p = Start-Process -FilePath powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{}'; exit $p.ExitCode",
        encode_command(script)
    );
    let mut command = std::process::Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-EncodedCommand",
        &encode_command(&launcher),
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command.status()?.success())
}

fn message(app: &AppHandle, kind: MessageDialogKind, text: &str) {
    app.dialog()
        .message(text)
        .title("Omnilogue Server")
        .kind(kind)
        .blocking_show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_subnets_and_addresses() {
        assert!(is_valid_remote_address("LocalSubnet"));
        assert!(is_valid_remote_address("192.168.50.184"));
        assert!(is_valid_remote_address("192.168.1.0/24, 10.0.0.5"));
        assert!(is_valid_remote_address("fe80::1"));
    }

    #[test]
    fn rejects_injection() {
        assert!(!is_valid_remote_address(""));
        assert!(!is_valid_remote_address("1.2.3.4'; Remove-Item C:\\"));
        assert!(!is_valid_remote_address("Any"));
        assert!(!is_valid_remote_address("1.2.3.4,"));
    }

    #[test]
    fn quotes_paths_with_apostrophes() {
        let script = rule_script(r"C:\Users\O'Brien\cchv-server.exe", 3727, "LocalSubnet");
        assert!(script.contains(r"-Program 'C:\Users\O''Brien\cchv-server.exe'"));
        assert!(script.contains("-RemoteAddress 'LocalSubnet'"));
        assert!(script.contains("-LocalPort 3727"));
    }

    #[test]
    fn encodes_utf16le() {
        assert_eq!(encode_command("a"), "YQA=");
    }
}
