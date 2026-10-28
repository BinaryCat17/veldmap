#![cfg(not(target_arch = "wasm32"))]

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

const GUEST_TIMEOUT: Duration = Duration::from_secs(120);

type TestResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[test]
fn wasi_http_tls_range_stream_cancellation_and_policy() -> TestResult<()> {
    let workspace = workspace_root()?;
    build_components(&workspace)?;
    let fixture = Fixture::start()?;
    let temporary = temporary_directory(&workspace)?;
    fs::create_dir_all(&temporary)?;
    fs::write(temporary.join("ca.pem"), fixture.ca_pem.as_bytes())?;
    let policy = format!(
        "[components.http-client]\nimports = []\nsocket_addresses = []\negress_origins = [\"{}\"]\nadditional_tls_roots = [\"ca.pem\"]\n",
        fixture.origin()
    );
    let policy_path = temporary.join("component-imports.toml");
    fs::write(&policy_path, policy)?;

    let target = target_dir(&workspace);
    let host = target.join("debug/veldmap-host");
    let component = target.join("wasm32-wasip3/debug/examples/http-client.wasm");
    let denied_origin = format!("https://127.0.0.1:{}", fixture.port());
    let mut guest = Guest::spawn(
        &host,
        &policy_path,
        &component,
        &[fixture.origin(), denied_origin],
        &workspace,
    )?;
    guest.expect_marker("HTTP_TLS_RANGE_STREAM_OK", GUEST_TIMEOUT)?;
    guest.expect_marker("HTTP_TOKIO_TIMER_OK", GUEST_TIMEOUT)?;
    guest.expect_marker("HTTP_REDIRECT_REJECTED", GUEST_TIMEOUT)?;
    guest.expect_marker("HTTP_BODY_DROPPED", GUEST_TIMEOUT)?;
    guest.expect_marker("HTTP_EGRESS_DENIED", GUEST_TIMEOUT)?;
    guest.wait_success(Duration::from_secs(30))?;
    fixture.cancelled.recv_timeout(Duration::from_secs(10))??;
    assert_eq!(fixture.redirect_hits.load(Ordering::SeqCst), 0);
    if let Ok(error) = fixture.errors.try_recv() {
        return error;
    }
    fixture.stop();
    fs::remove_dir_all(temporary)?;
    Ok(())
}

struct Fixture {
    port: u16,
    ca_pem: String,
    stop_flag: Arc<AtomicBool>,
    redirect_hits: Arc<AtomicUsize>,
    cancelled: Receiver<TestResult<()>>,
    errors: Receiver<TestResult<()>>,
    server: Option<JoinHandle<TestResult<()>>>,
}

impl Fixture {
    fn start() -> TestResult<Self> {
        let ca_key = KeyPair::generate()?;
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_cert = ca_params.self_signed(&ca_key)?;
        let issuer = Issuer::from_params(&ca_params, ca_key);
        let server_key = KeyPair::generate()?;
        let server_params = CertificateParams::new(["localhost".to_owned()])?;
        let server_cert = server_params.signed_by(&server_key, &issuer)?;
        let config = Arc::new(
            ServerConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
                .with_safe_default_protocol_versions()?
                .with_no_client_auth()
                .with_single_cert(
                    vec![server_cert.der().clone()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
                )?,
        );
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let redirect_hits = Arc::new(AtomicUsize::new(0));
        let (cancelled_tx, cancelled) = channel();
        let (errors_tx, errors) = channel();
        let server = {
            let stop_flag = stop_flag.clone();
            let redirect_hits = redirect_hits.clone();
            thread::spawn(move || -> TestResult<()> {
                while !stop_flag.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let config = config.clone();
                            let redirect_hits = redirect_hits.clone();
                            let cancelled_tx = cancelled_tx.clone();
                            let errors_tx = errors_tx.clone();
                            thread::spawn(move || {
                                if let Err(error) =
                                    handle_connection(stream, config, redirect_hits, cancelled_tx)
                                {
                                    let _ = errors_tx.send(Err(error));
                                }
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                Ok(())
            })
        };
        Ok(Self {
            port,
            ca_pem: ca_cert.pem(),
            stop_flag,
            redirect_hits,
            cancelled,
            errors,
            server: Some(server),
        })
    }

    fn port(&self) -> u16 {
        self.port
    }

    fn origin(&self) -> String {
        format!("https://localhost:{}", self.port)
    }

    fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn handle_connection(
    stream: TcpStream,
    config: Arc<ServerConfig>,
    redirect_hits: Arc<AtomicUsize>,
    cancelled: Sender<TestResult<()>>,
) -> TestResult<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let connection = ServerConnection::new(config)?;
    let mut stream = StreamOwned::new(connection, stream);
    let mut request = [0; 8192];
    let read = stream.read(&mut request)?;
    let request = String::from_utf8_lossy(&request[..read]);
    if request.starts_with("GET /range ") {
        if !request.contains("\r\nrange: bytes=4-11\r\n") {
            return Err("range header was not forwarded".into());
        }
        stream.write_all(
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 8\r\nContent-Range: bytes 4-11/16\r\nConnection: close\r\n\r\n4567",
        )?;
        stream.flush()?;
        thread::sleep(Duration::from_millis(20));
        stream.write_all(b"89ab")?;
    } else if request.starts_with("GET /redirect ") {
        if !request.contains("\r\nauthorization: AWS4-HMAC-SHA256 test\r\n") {
            return Err("signed request header was not forwarded".into());
        }
        stream.write_all(
            b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
    } else if request.starts_with("GET /final ") {
        redirect_hits.fetch_add(1, Ordering::SeqCst);
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
    } else if request.starts_with("GET /cancel ") {
        stream.write_all(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\n0123\r\n",
        )?;
        stream.flush()?;
        for _ in 0..250 {
            thread::sleep(Duration::from_millis(20));
            if stream
                .write_all(b"4\r\n4567\r\n")
                .and_then(|()| stream.flush())
                .is_err()
            {
                cancelled.send(Ok(()))?;
                return Ok(());
            }
        }
        return Err("response body was not cancelled".into());
    } else {
        return Err(format!("unexpected request: {request}").into());
    }
    stream.flush()?;
    Ok(())
}

struct Guest {
    child: Child,
    stdout: Receiver<String>,
    stdout_reader: Option<JoinHandle<String>>,
    stderr_reader: Option<JoinHandle<String>>,
}

impl Guest {
    fn spawn(
        host: &Path,
        policy: &Path,
        component: &Path,
        arguments: &[String],
        workspace: &Path,
    ) -> TestResult<Self> {
        let mut child = Command::new(host)
            .current_dir(workspace)
            .arg("--config")
            .arg(policy)
            .arg(component)
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("guest stdout was not piped");
        let stderr = child.stderr.take().expect("guest stderr was not piped");
        let (sender, receiver) = channel();
        Ok(Self {
            child,
            stdout: receiver,
            stdout_reader: Some(read_output(stdout, Some(sender))),
            stderr_reader: Some(read_output(stderr, None)),
        })
    }

    fn expect_marker(&self, expected: &str, timeout: Duration) -> TestResult<()> {
        let line = self.stdout.recv_timeout(timeout)?;
        if line == expected {
            Ok(())
        } else {
            Err(format!("expected {expected}, received {line}").into())
        }
    }

    fn wait_success(&mut self, timeout: Duration) -> TestResult<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return status
                    .success()
                    .then_some(())
                    .ok_or_else(|| format!("HTTP guest exited with {status}").into());
            }
            if Instant::now() >= deadline {
                return Err("HTTP guest did not exit".into());
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
            eprintln!("HTTP guest stdout:\n{stdout}");
        }
        if !stderr.is_empty() {
            eprintln!("HTTP guest stderr:\n{stderr}");
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

fn workspace_root() -> TestResult<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "failed to locate workspace".into())
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

fn temporary_directory(workspace: &Path) -> TestResult<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    Ok(target_dir(workspace)
        .join("tmp")
        .join(format!("wasi-http-{}-{nonce}", std::process::id())))
}

fn build_components(workspace: &Path) -> TestResult<()> {
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
            "http-client",
            "--features",
            "http-example",
        ])
        .output()?;
    if !guest.status.success() {
        return Err(format!(
            "HTTP guest build failed\n{}\n{}",
            String::from_utf8_lossy(&guest.stdout),
            String::from_utf8_lossy(&guest.stderr)
        )
        .into());
    }
    let host = Command::new(cargo)
        .current_dir(workspace)
        .args(["build", "--locked", "-p", "veldmap-host"])
        .output()?;
    if host.status.success() {
        Ok(())
    } else {
        Err(format!(
            "host build failed\n{}\n{}",
            String::from_utf8_lossy(&host.stdout),
            String::from_utf8_lossy(&host.stderr)
        )
        .into())
    }
}
