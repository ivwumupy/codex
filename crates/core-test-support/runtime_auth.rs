//! Route isolated first-party auth tests to loopback without relaxing destination checks.

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;
use wiremock::MockServer;

pub const PROBE_SERVER: &str = "CODEX_RUNTIME_AUTH_TEST_SERVER";

/// Run one test in an isolated subprocess with first-party HTTP routed to the mock server.
pub async fn run_harness_through_proxy(server: &MockServer, test_name: &str) -> Result<()> {
    run_harness_through_proxy_with_env(server, test_name, &[]).await
}

/// Adds process-local auth inputs without changing the test runner's environment.
pub async fn run_harness_through_proxy_with_env(
    server: &MockServer,
    test_name: &str,
    environment: &[(&str, Option<&str>)],
) -> Result<()> {
    // The production destination check stays intact. A test-only CA and CONNECT proxy route
    // the real host names to wiremock, only inside this isolated test subprocess.
    codex_utils_rustls_provider::ensure_rustls_crypto_provider();
    let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec![
        "api.openai.com".into(),
        "api.openai.org".into(),
        "auth.openai.com".into(),
        "chatgpt.com".into(),
        "us.chatgpt.com".into(),
        "chatgpt-staging.com".into(),
        "us.chatgpt-staging.com".into(),
        "auth.api.openai.org".into(),
    ])?;
    let ca_dir = tempfile::tempdir()?;
    let ca_path = ca_dir.path().join("ca.pem");
    std::fs::write(&ca_path, cert.pem())?;
    let acceptor = TlsAcceptor::from(Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], signing_key.into())?,
    ));
    let proxy = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_url = format!("http://{}", proxy.local_addr()?);
    let target = *server.address();
    let proxy_task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            let (mut stream, _) = proxy.accept().await?;
            let acceptor = acceptor.clone();
            connections.spawn(async move {
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") && headers.len() < 8192 {
                    headers.push(stream.read_u8().await?);
                }
                let headers = std::str::from_utf8(&headers)?;
                anyhow::ensure!(
                    matches!(
                        headers.lines().next(),
                        Some(
                            "CONNECT api.openai.com:443 HTTP/1.1"
                                | "CONNECT api.openai.org:443 HTTP/1.1"
                                | "CONNECT auth.openai.com:443 HTTP/1.1"
                                | "CONNECT chatgpt.com:443 HTTP/1.1"
                                | "CONNECT us.chatgpt.com:443 HTTP/1.1"
                                | "CONNECT chatgpt-staging.com:443 HTTP/1.1"
                                | "CONNECT us.chatgpt-staging.com:443 HTTP/1.1"
                                | "CONNECT auth.api.openai.org:443 HTTP/1.1"
                        )
                    ),
                    "unexpected proxy destination"
                );
                stream
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                let mut stream = acceptor.accept(stream).await?;
                let mut target = TcpStream::connect(target).await?;
                tokio::io::copy_bidirectional(&mut stream, &mut target).await?;
                Ok::<_, anyhow::Error>(())
            });
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    });
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--exact", test_name, "--nocapture"])
        .kill_on_drop(true);
    for &key in codex_network_proxy::PROXY_ENV_KEYS {
        command.env_remove(key);
    }
    command
        .env(PROBE_SERVER, server.uri())
        .env("HTTPS_PROXY", proxy_url)
        .env("NO_PROXY", codex_network_proxy::DEFAULT_NO_PROXY_VALUE)
        .env("CODEX_CA_CERTIFICATE", ca_path);
    for (key, value) in environment {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let output = tokio::time::timeout(Duration::from_secs(90), command.output()).await;
    proxy_task.abort();
    let output = output??;
    assert!(
        output.status.success(),
        "harness subprocess failed\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}
