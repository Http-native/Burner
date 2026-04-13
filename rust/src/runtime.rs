use crate::service::{Definition, path_to_string, timestamp};
use crate::store::Store;
use anyhow::{Context, Result, anyhow, bail};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub trait Manager {
    fn name(&self) -> &'static str;
    fn deploy(&self, def: &mut Definition, exec_path: Option<&str>) -> Result<()>;
    fn start(&self, def: &mut Definition) -> Result<()>;
    fn stop(&self, def: &mut Definition) -> Result<()>;
    fn delete(&self, def: &mut Definition) -> Result<()>;
    fn restart(&self, def: &mut Definition) -> Result<()> {
        self.stop(def)?;
        self.start(def)
    }
    fn status(&self, def: &Definition) -> Result<String>;
    fn logs(&self, def: &Definition, lines: usize) -> Result<Vec<u8>>;
}

pub fn default_manager() -> Box<dyn Manager> {
    if cfg!(target_os = "linux") && has_systemctl() {
        Box::new(SystemdManager)
    } else {
        Box::new(LocalManager)
    }
}

pub fn manager_for(def: &Definition) -> Result<Box<dyn Manager>> {
    match def.runtime.as_str() {
        "" | "local" => Ok(Box::new(LocalManager)),
        "systemd" => {
            if !cfg!(target_os = "linux") {
                bail!("systemd-managed service can only be controlled from Linux");
            }
            if !has_systemctl() {
                bail!("systemctl is not available on this machine");
            }
            Ok(Box::new(SystemdManager))
        }
        other => bail!("unknown runtime \"{other}\""),
    }
}

pub struct LocalManager;

impl Manager for LocalManager {
    fn name(&self) -> &'static str {
        "local"
    }

    fn deploy(&self, def: &mut Definition, _exec_path: Option<&str>) -> Result<()> {
        self.start(def)
    }

    fn start(&self, def: &mut Definition) -> Result<()> {
        if def.pid > 0 && process_running(def.pid) {
            bail!("service \"{}\" is already running with pid {}", def.name, def.pid);
        }

        let root = Store::default_root()?;
        let st = Store::new(root);
        st.init()?;

        let log_path = st.log_path(&def.name);
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .context("open log file")?;
        let stderr = stdout.try_clone().context("open log file")?;

        let mut cmd = Command::new(shell_command());
        cmd.arg(shell_arg())
            .arg(&def.command)
            .current_dir(&def.location)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .stdin(Stdio::null())
            .envs(std::env::vars());
        set_detached_process(&mut cmd);

        let child = cmd.spawn().context("start process")?;

        def.pid = child.id() as i32;
        def.runtime = self.name().into();
        def.status = "running".into();
        def.log_path = path_to_string(log_path);
        def.last_started_at = timestamp();
        Ok(())
    }

    fn stop(&self, def: &mut Definition) -> Result<()> {
        if def.pid == 0 {
            def.status = "stopped".into();
            return Ok(());
        }
        if !process_running(def.pid) {
            def.pid = 0;
            def.status = "stopped".into();
            return Ok(());
        }

        terminate_process(def.pid)?;
        def.pid = 0;
        def.status = "stopped".into();
        Ok(())
    }

    fn delete(&self, def: &mut Definition) -> Result<()> {
        self.stop(def)
    }

    fn status(&self, def: &Definition) -> Result<String> {
        if def.pid == 0 {
            return Ok("stopped".into());
        }
        if process_running(def.pid) {
            Ok("running".into())
        } else {
            Ok("exited".into())
        }
    }

    fn logs(&self, def: &Definition, lines: usize) -> Result<Vec<u8>> {
        let log_path = if def.log_path.is_empty() {
            Store::new(Store::default_root()?).log_path(&def.name)
        } else {
            PathBuf::from(&def.log_path)
        };
        tail_file(&log_path, lines)
    }
}

pub struct SystemdManager;

impl Manager for SystemdManager {
    fn name(&self) -> &'static str {
        "systemd"
    }

    fn deploy(&self, def: &mut Definition, exec_path: Option<&str>) -> Result<()> {
        let unit_name = systemd_unit_name(&def.name);
        let unit_path = PathBuf::from("/etc/systemd/system").join(&unit_name);
        let exec_path = match exec_path {
            Some(path) => path.to_string(),
            None => path_to_string(std::env::current_exe().context("find executable path")?),
        };

        def.unit_name = unit_name.clone();
        def.runtime = self.name().into();
        def.status = "installed".into();

        let root = Store::default_root()?;
        let unit_body = [
            "[Unit]".to_string(),
            format!("Description=Burner service {}", def.name),
            "After=network.target".into(),
            String::new(),
            "[Service]".into(),
            "Type=simple".into(),
            format!("WorkingDirectory={}", def.location),
            format!("Environment=BURNER_HOME={}", systemd_quote(&path_to_string(root))),
            format!(
                "ExecStart={} run --service {} --foreground",
                systemd_quote(&exec_path),
                systemd_quote(&def.name)
            ),
            "Restart=always".into(),
            "RestartSec=2".into(),
            String::new(),
            "[Install]".into(),
            "WantedBy=multi-user.target".into(),
            String::new(),
        ]
        .join("\n");

        fs::write(&unit_path, unit_body)
            .with_context(|| format!("write systemd unit {}", unit_path.display()))?;

        for args in [["daemon-reload"].as_slice(), ["enable", "--now", &unit_name].as_slice()] {
            let output = Command::new("systemctl")
                .args(args)
                .output()
                .with_context(|| format!("systemctl {}", args.join(" ")))?;
            if !output.status.success() {
                bail!(
                    "systemctl {}: {}",
                    args.join(" "),
                    combined_output(&output)
                );
            }
        }

        def.status = self.status(def)?;
        Ok(())
    }

    fn start(&self, def: &mut Definition) -> Result<()> {
        self.control(def, "start")
    }

    fn stop(&self, def: &mut Definition) -> Result<()> {
        self.control(def, "stop")
    }

    fn delete(&self, def: &mut Definition) -> Result<()> {
        let unit_name = if def.unit_name.is_empty() {
            systemd_unit_name(&def.name)
        } else {
            def.unit_name.clone()
        };
        let unit_path = PathBuf::from("/etc/systemd/system").join(&unit_name);

        let _ = self.control(def, "stop");
        let _ = run_systemctl_allow_missing(["disable", &unit_name].as_slice());
        if unit_path.exists() {
            fs::remove_file(&unit_path)
                .with_context(|| format!("remove systemd unit {}", unit_path.display()))?;
        }
        let _ = run_systemctl_allow_missing(["daemon-reload"].as_slice());
        let _ = run_systemctl_allow_missing(["reset-failed", &unit_name].as_slice());

        def.pid = 0;
        def.status = "deleted".into();
        Ok(())
    }

    fn restart(&self, def: &mut Definition) -> Result<()> {
        self.control(def, "restart")
    }

    fn status(&self, def: &Definition) -> Result<String> {
        let unit_name = if def.unit_name.is_empty() {
            systemd_unit_name(&def.name)
        } else {
            def.unit_name.clone()
        };

        let output = Command::new("systemctl")
            .args(["is-active", &unit_name])
            .output()
            .with_context(|| format!("systemctl is-active {unit_name}"))?;
        let status = combined_output(&output);
        if output.status.success() {
            Ok(status)
        } else if status.is_empty() {
            Err(anyhow!("systemctl is-active {unit_name}: command failed"))
        } else {
            Ok(status)
        }
    }

    fn logs(&self, def: &Definition, lines: usize) -> Result<Vec<u8>> {
        let unit_name = if def.unit_name.is_empty() {
            systemd_unit_name(&def.name)
        } else {
            def.unit_name.clone()
        };

        let output = Command::new("journalctl")
            .args(["-u", &unit_name, "-n", &lines.to_string(), "--no-pager"])
            .output()
            .with_context(|| format!("journalctl {unit_name}"))?;
        if !output.status.success() {
            bail!("journalctl {}: {}", unit_name, combined_output(&output));
        }
        Ok(output.stdout)
    }
}

impl SystemdManager {
    fn control(&self, def: &mut Definition, action: &str) -> Result<()> {
        let unit_name = if def.unit_name.is_empty() {
            systemd_unit_name(&def.name)
        } else {
            def.unit_name.clone()
        };

        let output = Command::new("systemctl")
            .args([action, &unit_name])
            .output()
            .with_context(|| format!("systemctl {action} {unit_name}"))?;
        if !output.status.success() {
            bail!(
                "systemctl {} {}: {}",
                action,
                unit_name,
                combined_output(&output)
            );
        }

        def.status = self.status(def)?;
        if action == "stop" {
            def.pid = 0;
        }
        Ok(())
    }
}

fn run_systemctl_allow_missing(args: &[&str]) -> Result<()> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .with_context(|| format!("systemctl {}", args.join(" ")))?;
    if output.status.success() {
        return Ok(());
    }

    let text = combined_output(&output);
    let lowered = text.to_lowercase();
    if lowered.contains("not loaded")
        || lowered.contains("does not exist")
        || lowered.contains("no such file")
        || lowered.contains("not-found")
    {
        return Ok(());
    }

    bail!("systemctl {}: {}", args.join(" "), text)
}

pub fn run_command_foreground(dir: &str, command: &str) -> Result<()> {
    run_command_foreground_impl(dir, command)
}

#[cfg(unix)]
fn run_command_foreground_impl(dir: &str, command: &str) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    std::env::set_current_dir(dir).context("change directory")?;

    let shell = CString::new("/bin/sh")?;
    let arg1 = CString::new("-lc")?;
    let arg2 = CString::new(command)?;
    let args = [shell.as_ptr(), arg1.as_ptr(), arg2.as_ptr(), std::ptr::null()];

    let mut env_storage = Vec::new();
    for (key, value) in std::env::vars_os() {
        let mut bytes = key.as_os_str().as_bytes().to_vec();
        bytes.push(b'=');
        bytes.extend_from_slice(value.as_os_str().as_bytes());
        env_storage.push(CString::new(bytes)?);
    }
    let mut env_ptrs: Vec<*const libc::c_char> = env_storage.iter().map(|v| v.as_ptr()).collect();
    env_ptrs.push(std::ptr::null());

    // SAFETY: pointers stay valid for the duration of execve call; on success this process is replaced.
    let rc = unsafe { libc::execve(shell.as_ptr(), args.as_ptr(), env_ptrs.as_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("run command");
    }
    Ok(())
}

#[cfg(not(unix))]
fn run_command_foreground_impl(dir: &str, command: &str) -> Result<()> {
    let status = Command::new("cmd")
        .args(["/C", command])
        .current_dir(dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("run command")?;
    if status.success() {
        Ok(())
    } else {
        bail!("run command: exited with status {status}");
    }
}

pub fn tail_file(path: &PathBuf, lines: usize) -> Result<Vec<u8>> {
    let lines = if lines == 0 { 100 } else { lines };
    let file = fs::File::open(path).with_context(|| format!("open log file {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut buffer = Vec::new();
    for line in reader.lines() {
        buffer.push(line.with_context(|| format!("read log file {}", path.display()))?);
        if buffer.len() > lines {
            buffer.remove(0);
        }
    }
    if buffer.is_empty() {
        return Ok(Vec::new());
    }
    Ok(format!("{}\n", buffer.join("\n")).into_bytes())
}

fn shell_command() -> &'static str {
    if cfg!(windows) { "cmd" } else { "/bin/sh" }
}

fn shell_arg() -> &'static str {
    if cfg!(windows) { "/C" } else { "-lc" }
}

fn has_systemctl() -> bool {
    Command::new("systemctl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn systemd_unit_name(name: &str) -> String {
    format!("burner-{name}.service")
}

fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn combined_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
        (true, true) => String::new(),
    }
}

#[cfg(unix)]
fn set_detached_process(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: this runs in the forked child before exec and only calls async-signal-safe setpgid.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn set_detached_process(_cmd: &mut Command) {}

#[cfg(unix)]
fn process_running(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill with signal 0 only queries process existence.
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(code) if code == libc::ESRCH => false,
        Some(_) => true,
        None => false,
    }
}

#[cfg(not(unix))]
fn process_running(_pid: i32) -> bool {
    false
}

#[cfg(unix)]
fn terminate_process(pid: i32) -> Result<()> {
    // SAFETY: pid comes from stored process metadata.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(err).context("signal process");
        }
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if !process_running(pid) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(150));
    }

    if process_running(pid) {
        // SAFETY: pid comes from stored process metadata.
        let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err).context("force kill process");
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn terminate_process(_pid: i32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::tail_file;
    use std::fs;

    #[test]
    fn tails_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service.log");
        fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        let output = tail_file(&path, 2).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "three\nfour\n");
    }
}
