use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

const LABEL: &str = "com.grug-brain.server";

/// Returns the path where the service file should be written.
/// macOS: ~/Library/LaunchAgents/com.grug-brain.server.plist
/// Linux: ~/.config/systemd/user/grug-brain.service
fn service_file_path() -> Result<PathBuf, String> {
    let home =
        env::var("HOME").map_err(|_| "grug: HOME environment variable not set".to_string())?;

    if cfg!(target_os = "macos") {
        Ok(PathBuf::from(&home)
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    } else if cfg!(target_os = "linux") {
        Ok(PathBuf::from(&home)
            .join(".config/systemd/user")
            .join("grug-brain.service"))
    } else {
        Err("grug: unsupported platform (only macOS and Linux are supported)".to_string())
    }
}

/// Find the grug binary path.
/// Uses current_exe() which resolves to the actual binary location.
fn grug_binary_path() -> Result<PathBuf, String> {
    env::current_exe().map_err(|e| format!("grug: failed to determine binary path: {e}"))
}

/// Get the current user's UID (for launchctl gui/ domain).
fn current_uid() -> Result<String, String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| format!("grug: failed to get UID: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err("grug: `id -u` failed".to_string())
    }
}

/// Generate macOS launchd plist XML content.
pub fn generate_plist(binary_path: &Path, socket_path: Option<&Path>) -> String {
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let grug_home = PathBuf::from(&home).join(".grug-brain");
    let bin = binary_path.display();

    let mut args = format!(
        "    <array>\n      <string>{bin}</string>\n      <string>serve</string>\n"
    );
    if let Some(sock) = socket_path {
        args.push_str(&format!(
            "      <string>--socket</string>\n      <string>{}</string>\n",
            sock.display()
        ));
    }
    args.push_str("    </array>");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
{args}
  <key>KeepAlive</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{stdout}</string>
  <key>StandardErrorPath</key>
  <string>{stderr}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>{home}</string>
  </dict>
</dict>
</plist>
"#,
        stdout = grug_home.join("launchd-stdout.log").display(),
        stderr = grug_home.join("launchd-stderr.log").display(),
    )
}

/// Generate Linux systemd unit file content.
pub fn generate_systemd_unit(binary_path: &Path, socket_path: Option<&Path>) -> String {
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());

    let mut exec_start = format!("{} serve", binary_path.display());
    if let Some(sock) = socket_path {
        exec_start.push_str(&format!(" --socket {}", sock.display()));
    }

    format!(
        r#"[Unit]
Description=grug-brain memory server
After=network.target

[Service]
Type=simple
ExecStart={exec_start}
Restart=always
RestartSec=5
Environment=HOME={home}

[Install]
WantedBy=default.target
"#
    )
}

/// Run a command, returning its stdout on success or an error message on failure.
fn run_cmd(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).output().map_err(|e| {
        format!("grug: failed to run {program} {}: {e}", args.join(" "))
    })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "grug: {program} {} failed: {stderr}",
            args.join(" ")
        ))
    }
}

/// Install and enable the grug-brain service.
///
/// On macOS: writes a launchd plist and bootstraps it.
/// On Linux: writes a systemd user unit and enables it.
pub fn install_service(socket_path: Option<&Path>) -> Result<(), String> {
    let binary = grug_binary_path()?;
    let service_path = service_file_path()?;

    // Ensure parent directory exists
    if let Some(parent) = service_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("grug: failed to create service directory: {e}"))?;
    }

    // Ensure ~/.grug-brain exists
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let grug_home = PathBuf::from(&home).join(".grug-brain");
    fs::create_dir_all(&grug_home)
        .map_err(|e| format!("grug: failed to create ~/.grug-brain: {e}"))?;

    if cfg!(target_os = "macos") {
        install_macos(&binary, &service_path, socket_path)
    } else if cfg!(target_os = "linux") {
        install_linux(&binary, &service_path, socket_path)
    } else {
        Err("grug: unsupported platform".to_string())
    }
}

fn install_macos(
    binary: &Path,
    service_path: &Path,
    socket_path: Option<&Path>,
) -> Result<(), String> {
    let uid = current_uid()?;
    let domain = format!("gui/{uid}");
    let service_str = service_path
        .to_str()
        .ok_or("grug: invalid service path")?;

    // Unload existing service (ignore errors — may not be loaded)
    let _ = run_cmd("launchctl", &["bootout", &domain, service_str]);

    // Write plist file
    let content = generate_plist(binary, socket_path);
    fs::write(service_path, &content)
        .map_err(|e| format!("grug: failed to write plist: {e}"))?;

    // Load the new service
    run_cmd("launchctl", &["bootstrap", &domain, service_str]).map_err(|e| {
        format!(
            "{e}\nPlist written to {service_str} but failed to load.\n\
             Try manually: launchctl bootstrap {domain} {service_str}"
        )
    })?;

    // Verify
    let list_output = run_cmd("launchctl", &["list"]).unwrap_or_default();
    if list_output.contains(LABEL) {
        eprintln!("grug: service installed and running");
        eprintln!("  plist: {service_str}");
        eprintln!("  stop:  launchctl bootout {domain} {service_str}");
        eprintln!("  logs:  ~/.grug-brain/launchd-stderr.log");
        Ok(())
    } else {
        Err(format!(
            "grug: service installed but not listed in launchctl.\n\
             Check logs: ~/.grug-brain/launchd-stderr.log"
        ))
    }
}

fn install_linux(
    binary: &Path,
    service_path: &Path,
    socket_path: Option<&Path>,
) -> Result<(), String> {
    // Write unit file
    let content = generate_systemd_unit(binary, socket_path);
    fs::write(service_path, &content)
        .map_err(|e| format!("grug: failed to write systemd unit: {e}"))?;

    // Reload, enable, start
    run_cmd("systemctl", &["--user", "daemon-reload"])?;
    run_cmd("systemctl", &["--user", "enable", "grug-brain.service"])?;
    run_cmd("systemctl", &["--user", "restart", "grug-brain.service"])?;

    // Enable linger (survive logout)
    if let Ok(user) = env::var("USER") {
        let _ = run_cmd("loginctl", &["enable-linger", &user]);
    }

    // Verify
    let status = run_cmd("systemctl", &["--user", "is-enabled", "grug-brain.service"])
        .unwrap_or_default();
    let service_str = service_path.display();
    if status.trim() == "enabled" {
        eprintln!("grug: service installed and enabled");
        eprintln!("  unit:    {service_str}");
        eprintln!("  stop:    systemctl --user stop grug-brain.service");
        eprintln!("  restart: systemctl --user restart grug-brain.service");
        eprintln!("  logs:    journalctl --user -u grug-brain.service");
        Ok(())
    } else {
        Err(format!(
            "grug: service installed but not enabled.\n\
             Check: journalctl --user -u grug-brain.service"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_generate_plist_basic() {
        let plist = generate_plist(Path::new("/usr/local/bin/grug"), None);
        assert!(plist.contains("<string>com.grug-brain.server</string>"));
        assert!(plist.contains("<string>/usr/local/bin/grug</string>"));
        assert!(plist.contains("<string>serve</string>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<true/>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("launchd-stdout.log"));
        assert!(plist.contains("launchd-stderr.log"));
        assert!(plist.contains("<key>HOME</key>"));
        // Should NOT contain --socket when None
        assert!(!plist.contains("--socket"));
    }

    #[test]
    fn test_generate_plist_custom_socket() {
        let plist =
            generate_plist(Path::new("/opt/bin/grug"), Some(Path::new("/tmp/grug.sock")));
        assert!(plist.contains("<string>--socket</string>"));
        assert!(plist.contains("<string>/tmp/grug.sock</string>"));
        assert!(plist.contains("<string>/opt/bin/grug</string>"));
        assert!(plist.contains("<string>serve</string>"));
    }

    #[test]
    fn test_generate_systemd_unit_basic() {
        let unit = generate_systemd_unit(Path::new("/usr/local/bin/grug"), None);
        assert!(unit.contains("Description=grug-brain memory server"));
        assert!(unit.contains("ExecStart=/usr/local/bin/grug serve"));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("RestartSec=5"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("Environment=HOME="));
        // Should NOT contain --socket when None
        assert!(!unit.contains("--socket"));
    }

    #[test]
    fn test_generate_systemd_unit_custom_socket() {
        let unit = generate_systemd_unit(
            Path::new("/opt/bin/grug"),
            Some(Path::new("/tmp/grug.sock")),
        );
        assert!(unit.contains("ExecStart=/opt/bin/grug serve --socket /tmp/grug.sock"));
    }

    #[test]
    fn test_plist_is_valid_xml() {
        let plist = generate_plist(Path::new("/usr/local/bin/grug"), None);
        assert!(plist.starts_with("<?xml version=\"1.0\""));
        assert!(plist.contains("<!DOCTYPE plist"));
        assert!(plist.contains("<plist version=\"1.0\">"));
        assert!(plist.trim_end().ends_with("</plist>"));
    }

    #[test]
    fn test_systemd_unit_has_all_sections() {
        let unit = generate_systemd_unit(Path::new("/usr/local/bin/grug"), None);
        assert!(unit.contains("[Unit]"));
        assert!(unit.contains("[Service]"));
        assert!(unit.contains("[Install]"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_service_file_path_macos() {
        let path = service_file_path().unwrap();
        let path_str = path.to_str().unwrap();
        assert!(path_str.contains("Library/LaunchAgents"));
        assert!(path_str.ends_with("com.grug-brain.server.plist"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_service_file_path_linux() {
        let path = service_file_path().unwrap();
        let path_str = path.to_str().unwrap();
        assert!(path_str.contains(".config/systemd/user"));
        assert!(path_str.ends_with("grug-brain.service"));
    }
}
