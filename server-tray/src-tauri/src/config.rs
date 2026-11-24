//! Settings for the server the tray runs, stored as `server.json` in the app's
//! config directory (`%APPDATA%\com.claude.history-viewer.server`).
//!
//! The file is read again every time the server starts, so "Restart server"
//! from the tray menu applies edits without restarting the tray.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "server.json";
pub const DEFAULT_PORT: u16 = 3727;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    /// Address the server binds to. `0.0.0.0` listens on every interface;
    /// `127.0.0.1` keeps it on this PC only.
    pub host: String,
    pub port: u16,
    /// Access token the web UI asks for. Passed to the server in `CCHV_TOKEN`,
    /// never on the command line. Generated on first run.
    pub token: String,
    /// Address phones and other devices use, e.g. a reverse proxy
    /// (`https://history.example.com`). Used for "Copy access link".
    /// Empty means `http://<this PC's LAN address>:<port>`.
    pub public_url: String,
    /// Extra arguments for the server, e.g. `["--read-only"]`.
    pub extra_args: Vec<String>,
    /// Check GitHub for a new version at startup and once a day.
    pub check_for_updates: bool,
    /// Who the "Allow through Windows Firewall" rule admits: `LocalSubnet`,
    /// or addresses such as `192.168.1.10` or `192.168.1.0/24`, comma-separated.
    pub firewall_remote_address: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: DEFAULT_PORT,
            token: String::new(),
            public_url: String::new(),
            extra_args: Vec::new(),
            check_for_updates: true,
            firewall_remote_address: "LocalSubnet".into(),
        }
    }
}

impl ServerConfig {
    /// True when the server is reachable from other machines.
    pub fn is_network_exposed(&self) -> bool {
        !matches!(self.host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }

    /// URL for opening the UI in a browser on this PC, logged in.
    pub fn local_url(&self) -> String {
        let host = match self.host.as_str() {
            "0.0.0.0" | "::" | "" => "127.0.0.1",
            other => other,
        };
        with_token(
            &format!("http://{}", host_port(host, self.port)),
            &self.token,
        )
    }

    /// URL to give a phone or another device, logged in.
    pub fn access_url(&self) -> String {
        let base = if self.public_url.trim().is_empty() {
            let host = match self.host.as_str() {
                "0.0.0.0" | "::" | "" => {
                    lan_ip().map_or_else(|| "127.0.0.1".into(), |ip| ip.to_string())
                }
                other => other.to_string(),
            };
            format!("http://{}", host_port(&host, self.port))
        } else {
            self.public_url.trim().to_string()
        };
        with_token(&base, &self.token)
    }

    /// The address shown in the tray, without the token.
    pub fn display_address(&self) -> String {
        let url = self.access_url();
        url.split("?token=")
            .next()
            .unwrap_or(&url)
            .trim_end_matches('/')
            .to_string()
    }
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn with_token(base: &str, token: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/?token={token}")
}

/// This PC's address on the network that holds the default route. Connecting a
/// UDP socket only picks a route; no packet is sent.
pub fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// A 256-bit random token, base64url without padding (43 characters).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Reads the settings, creating the file with defaults and a new token when it
/// does not exist. Returns the settings and whether this is the first run.
pub fn load_or_create(dir: &Path) -> io::Result<(ServerConfig, bool)> {
    let path = path_in(dir);
    let (mut config, created) = match fs::read_to_string(&path) {
        Ok(text) => {
            let config = serde_json::from_str::<ServerConfig>(&text).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?;
            (config, false)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (ServerConfig::default(), true),
        Err(e) => return Err(e),
    };
    if created || config.token.trim().is_empty() {
        if config.token.trim().is_empty() {
            config.token = generate_token();
        }
        save(dir, &config)?;
    }
    Ok((config, created))
}

pub fn save(dir: &Path, config: &ServerConfig) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = path_in(dir);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
    fs::write(&tmp, text + "\n")?;
    fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_long_and_url_safe() {
        let token = generate_token();
        assert_eq!(token.len(), 43);
        assert!(token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(token, generate_token());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let config: ServerConfig = serde_json::from_str(r#"{"port": 4000}"#).unwrap();
        assert_eq!(config.port, 4000);
        assert_eq!(config.host, "0.0.0.0");
        assert!(config.check_for_updates);
    }

    #[test]
    fn urls_carry_the_token() {
        let config = ServerConfig {
            token: "abc".into(),
            public_url: "https://history.example.com/".into(),
            ..ServerConfig::default()
        };
        assert_eq!(config.local_url(), "http://127.0.0.1:3727/?token=abc");
        assert_eq!(
            config.access_url(),
            "https://history.example.com/?token=abc"
        );
        assert_eq!(config.display_address(), "https://history.example.com");
    }

    #[test]
    fn ipv6_hosts_are_bracketed() {
        let config = ServerConfig {
            host: "::1".into(),
            token: "t".into(),
            ..ServerConfig::default()
        };
        assert_eq!(config.local_url(), "http://[::1]:3727/?token=t");
        assert!(!config.is_network_exposed());
    }

    #[test]
    fn first_run_creates_file_with_token() {
        let dir = std::env::temp_dir().join(format!("omnilogue-tray-test-{}", generate_token()));
        let (config, created) = load_or_create(&dir).unwrap();
        assert!(created);
        assert_eq!(config.token.len(), 43);
        let (again, created_again) = load_or_create(&dir).unwrap();
        assert!(!created_again);
        assert_eq!(again, config);
        fs::remove_dir_all(dir).unwrap();
    }
}
