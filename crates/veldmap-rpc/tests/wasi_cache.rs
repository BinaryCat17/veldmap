#![cfg(not(target_arch = "wasm32"))]

use std::env;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[test]
fn cache_components_stream_and_cancel() -> Result<(), String> {
    let workspace = workspace_root()?;
    build_examples(&workspace)?;
    let target = target_dir(&workspace);
    let components = target.join("wasm32-wasip3/debug/examples");
    let host = target.join("debug/veldmap-host");

    let server = Guest::spawn(
        "cache-server",
        &host,
        &components.join("cache-server.wasm"),
        None,
        &workspace,
    )?;
    let address = server.ready(Duration::from_secs(120))?;
    let mut client = Guest::spawn(
        "cache-client",
        &host,
        &components.join("cache-client.wasm"),
        Some(address),
        &workspace,
    )?;

    client.expect_marker("CACHE_STREAM_OK", Duration::from_secs(120))?;
    client.expect_marker("CACHE_TIMER_OK", Duration::from_secs(120))?;
    client.expect_marker("CACHE_CANCEL_OK", Duration::from_secs(120))?;
    client.wait_success(Duration::from_secs(10))?;
    server.expect_marker("PRODUCER_TERMINATED", Duration::from_secs(15))?;
    Ok(())
}

fn workspace_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "failed to locate the workspace root".to_owned())
}

fn build_examples(workspace: &Path) -> Result<(), String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let guest = Command::new(&cargo)
        .current_dir(workspace)
        .args([
            "build",
            "--locked",
            "-p",
            "veldmap-rpc",
            "--target",
            "wasm32-wasip3",
            "--example",
            "cache-server",
            "--example",
            "cache-client",
        ])
        .output()
        .map_err(|error| format!("failed to build WASI examples: {error}"))?;
    if !guest.status.success() {
        return Err(format!(
            "WASI example build failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&guest.stdout),
            String::from_utf8_lossy(&guest.stderr)
        ));
    }
    let host = Command::new(cargo)
        .current_dir(workspace)
        .args(["build", "--locked", "-p", "veldmap-host"])
        .output()
        .map_err(|error| format!("failed to build host: {error}"))?;
    if host.status.success() {
        return Ok(());
    }
    Err(format!(
        "host build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&host.stdout),
        String::from_utf8_lossy(&host.stderr)
    ))
}

fn target_dir(workspace: &Path) -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(
        || workspace.join("target"),
        |path| {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        },
    )
}

struct Guest {
    name: &'static str,
    child: Child,
    stdout: Receiver<String>,
    stdout_reader: Option<JoinHandle<String>>,
    stderr_reader: Option<JoinHandle<String>>,
}

impl Guest {
    fn spawn(
        name: &'static str,
        host: &Path,
        component: &Path,
        argument: Option<SocketAddr>,
        workspace: &Path,
    ) -> Result<Self, String> {
        let mut command = Command::new(host);
        command
            .current_dir(workspace)
            .arg(component)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(address) = argument {
            command.arg(address.to_string());
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to launch {name}: {error}"))?;
        let stdout = child.stdout.take().expect("guest stdout was not piped");
        let stderr = child.stderr.take().expect("guest stderr was not piped");
        let (sender, receiver) = std::sync::mpsc::channel();
        Ok(Self {
            name,
            child,
            stdout: receiver,
            stdout_reader: Some(read_output(stdout, Some(sender))),
            stderr_reader: Some(read_output(stderr, None)),
        })
    }

    fn ready(&self, timeout: Duration) -> Result<SocketAddr, String> {
        let line = self.next_stdout(timeout)?;
        let address = line.strip_prefix("READY ").ok_or_else(|| {
            format!(
                "{}: expected a readiness address, received {line:?}",
                self.name
            )
        })?;
        address.parse().map_err(|error| {
            format!(
                "{}: invalid readiness address {address:?}: {error}",
                self.name
            )
        })
    }

    fn expect_marker(&self, marker: &str, timeout: Duration) -> Result<(), String> {
        let line = self.next_stdout(timeout)?;
        if line == marker {
            Ok(())
        } else {
            Err(format!(
                "{}: expected {marker:?}, received {line:?}",
                self.name
            ))
        }
    }

    fn next_stdout(&self, timeout: Duration) -> Result<String, String> {
        self.stdout
            .recv_timeout(timeout)
            .map_err(|error| format!("{}: waiting for guest output: {error}", self.name))
    }

    fn wait_success(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|error| format!("{}: failed to poll guest: {error}", self.name))?
            {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("{}: guest exited with {status}", self.name))
                };
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{}: guest did not exit within {timeout:?}",
                    self.name
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let stdout = join_output(self.stdout_reader.take());
        let stderr = join_output(self.stderr_reader.take());
        if !stdout.is_empty() {
            eprintln!("{} stdout:\n{stdout}", self.name);
        }
        if !stderr.is_empty() {
            eprintln!("{} stderr:\n{stderr}", self.name);
        }
    }
}

fn read_output<R>(reader: R, sender: Option<Sender<String>>) -> JoinHandle<String>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut captured = String::new();
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => {
                    captured.push_str(&line);
                    captured.push('\n');
                    if sender
                        .as_ref()
                        .is_some_and(|sender| sender.send(line).is_err())
                    {
                        break;
                    }
                }
                Err(error) => {
                    captured.push_str(&format!("output read failed: {error}\n"));
                    break;
                }
            }
        }
        captured
    })
}

fn join_output(reader: Option<JoinHandle<String>>) -> String {
    reader.map_or_else(String::new, |reader| {
        reader
            .join()
            .unwrap_or_else(|_| "output reader panicked\n".to_owned())
    })
}
