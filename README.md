# Omnilogue Server for Windows

A Windows installer and tray app for the [Omnilogue](https://github.com/jhlee0409/omnilogue)
web UI server. With it you can browse your Claude Code, Codex and other AI coding
session history from a phone or another computer, while the history stays on
your PC.

Upstream ships the headless server (`--serve`) for Linux and macOS. This project
builds the same server, unchanged, for Windows and wraps it in:

- an NSIS installer for the current user (no administrator rights needed)
- a tray icon that starts the server, restarts it if it exits, and stops it on Quit
- **Start with Windows**, turned on at first run (can be turned off from the menu)
- **Check for updates** and **Update now**, with signed updates from this repo's releases
- a random 256-bit access token, created at first run
- an optional Windows Firewall rule, added only after you agree

This is an unofficial, community package. Please report problems with the server
itself upstream, and problems with the installer or tray app here.

## Install

Download `OmnilogueServer_<version>_x64-setup.exe` from
[Releases](../../releases) and run it. The tray icon appears and the web UI
opens in your browser, already logged in.

To use it from your phone, choose **Copy access link** from the tray menu and
open the link on the phone. The link contains the token, so treat it like a
password.

## Tray menu

| Item | What it does |
| --- | --- |
| Status line | Running address, or why the server is not running |
| Open in browser | Opens the UI on this PC (starts the server if stopped) |
| Copy access link | Copies the address and token for another device |
| Stop / Start server | Stops or starts the server; the tray stays open |
| Restart server | Restarts it and re-reads the settings |
| Start with Windows | Starts the tray app when you sign in |
| Check for updates (current vX) | Looks for a newer release now |
| Update now to vY | Installs the update the last check found |
| Edit settings… | Opens `server.json` in Notepad; then choose Restart server |
| Open logs folder | Server and tray log (`server.log`) |
| Allow through Windows Firewall… | Adds an inbound rule (asks for admin rights) |
| Quit | Stops the server and closes the tray app |

## Settings

`%APPDATA%\com.claude.history-viewer.server\server.json`:

```json
{
  "host": "0.0.0.0",
  "port": 3727,
  "token": "<generated at first run>",
  "publicUrl": "",
  "extraArgs": [],
  "checkForUpdates": true,
  "firewallRemoteAddress": "LocalSubnet"
}
```

- `host`: `0.0.0.0` listens on every network interface. Use `127.0.0.1` to keep it on this PC only, or a single LAN address.
- `token`: the access token. Delete the value and restart the server to get a new one.
- `publicUrl`: the address other devices use, if you put the server behind a reverse proxy (e.g. `https://history.example.com`). **Copy access link** uses it.
- `extraArgs`: extra server flags, e.g. `["--read-only"]` or `["--base-path", "/history"]`.
- `firewallRemoteAddress`: who the firewall rule admits: `LocalSubnet`, or addresses and ranges such as `192.168.1.10, 192.168.1.0/24`.

The token is passed to the server in the `CCHV_TOKEN` environment variable, so it
doesn't appear in the process list.

## Security notes

- The server gives read access to your AI session history, which can contain
  code, file paths and secrets you pasted into prompts. Keep the token private.
- The server speaks plain HTTP. Off your home network, put it behind a reverse
  proxy with HTTPS (and ideally its own login), or use a VPN such as Tailscale.
  Don't forward the port straight to the internet.
- Keep `firewallRemoteAddress` as narrow as you can. If only a reverse proxy
  needs to reach the server, use the proxy's address.

## Building

Requirements: Windows 10/11, git, Node.js 20+, Rust (MSVC toolchain). pnpm is
used if it's installed; otherwise the build runs the pinned version through `npx`.

```powershell
./scripts/build.ps1                  # latest upstream release
./scripts/build.ps1 -Tag v2.1.1      # a specific release
```

The installer ends up in `dist\`. To produce signed updates, set
`TAURI_SIGNING_PRIVATE_KEY` (and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) first;
the build then also writes `latest-server-tray.json`.

### Releases

`.github/workflows/release.yml` runs daily. When upstream publishes a new release,
it builds the installer for that tag and publishes it here under the same tag.
It needs two repository secrets: `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The matching public key is in
`server-tray/src-tauri/tauri.conf.json`.

To set up a fork:

1. Generate a key pair: `cd server-tray; pnpm tauri signer generate -w $HOME\.tauri\omnilogue-server-windows.key`
2. Put the public key in `plugins.updater.pubkey` in `tauri.conf.json`.
3. Add the private key and its password as the two secrets above.
4. Run the Release workflow from the Actions tab.

## Layout

```
server-tray/            Tauri 2 tray app (no windows) + NSIS installer config
  src-tauri/src/
    config.rs           server.json: defaults, token, URLs
    supervisor.rs       runs cchv-server.exe, restarts it with backoff
    tray.rs             tray menu
    updates.rs          update checks and install
    firewall.rs         optional inbound firewall rule
scripts/build.ps1       builds upstream server + tray app + installer
UPSTREAM.md             how this could move into the Omnilogue repo
```

## License

MIT. The bundled server and the icons are from Omnilogue, © JaeHyeok Lee, MIT.
See [LICENSE](LICENSE).
