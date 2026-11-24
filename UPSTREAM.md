# Moving this into Omnilogue

This repo is built so the Omnilogue maintainers can adopt any part of it without
taking on the rest. It doesn't patch the upstream code. It builds the release
tag as-is with `--features webui-server` and keeps the tray app in one
self-contained folder (`server-tray/`).

Each path below works without the others.

## 1. Publish a Windows server binary (smallest change)

`server-release.yml` already builds the headless server for Linux and macOS.
Adding a Windows target to its matrix gives Windows users a plain
`cchv-server.exe`, which is all a scheduled task or a service wrapper needs:

```yaml
- os: windows-latest
  target: x86_64-pc-windows-msvc
  artifact: omnilogue-server-windows-x64.exe
```

The binary builds and runs on Windows today. The packaging in this repo has used
v2.1.1 built this way, with no source changes.

## 2. Ship the tray app from the main repo

Move `server-tray/` into the Omnilogue repo as-is, e.g. as
`packaging/windows-server-tray/`, and add a release job that:

1. builds the server as in path 1;
2. copies it to `server-tray/src-tauri/binaries/cchv-server-<triple>.exe`;
3. runs `pnpm tauri build --config <overlay>` with the release version (see
   `scripts/build.ps1`, which does exactly this);
4. uploads the installer, its `.sig` and `latest-server-tray.json` to the same
   GitHub release as the desktop app.

Things that are already set up to fit in:

- **Signing key.** The tray app uses the standard Tauri updater, so the existing
  `TAURI_SIGNING_PRIVATE_KEY` secret can sign it. Put the desktop app's public key
  in `server-tray/src-tauri/tauri.conf.json`.
- **Separate update manifest.** The tray app reads `latest-server-tray.json`, not
  `latest.json`, so desktop and server updates never get mixed up on the same
  release.
- **Separate identity.** Its identifier is `com.claude.history-viewer.server`, so
  it installs, autostarts and stores settings apart from the desktop app, and
  both can be installed side by side.
- **Versions.** The installer version is the upstream release version.
- **Dependencies.** Tauri and the plugins both apps use (dialog, opener,
  single-instance, updater) are pinned to the desktop app's versions. Only
  autostart and clipboard-manager are new.

`server-tray/src-tauri` is its own crate (not part of a workspace), so it doesn't
affect the desktop app's build or `Cargo.lock`.

## 3. Run the server in-process (optional refactor)

The tray app currently runs the server as a child process. That's because
`run_server` in `src-tauri/src/lib.rs` is private, reads `std::env::args`, calls
`process::exit` on startup errors, and stops only on Ctrl+C.

A small refactor would let the tray app, or a future "serve from the desktop
app" toggle, link the server directly:

```rust
pub struct ServerOptions { pub host: IpAddr, pub port: u16, pub token: Option<String>,
                           pub read_only: bool, pub base_path: String, pub dist: Option<PathBuf> }

pub struct ServerHandle { /* shutdown sender + join handle */ }
impl ServerHandle { pub fn shutdown(self) -> Result<(), ServerError>; }

pub async fn start_server(opts: ServerOptions) -> Result<ServerHandle, ServerError>;
```

`run_server` would then parse the CLI into `ServerOptions` and call
`start_server`, so the CLI keeps working the same. Axum's
`with_graceful_shutdown` already supports this shape. With it in place, the
tray app could drop the sidecar, the job object and the log redirection.
This is optional: the child-process design works, and it keeps the server
exactly as upstream builds it.
