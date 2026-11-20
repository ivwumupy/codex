//! Exercise authorized MITM forwarding over TLS, including transport-owned request bodies.

use super::*;
use crate::ProxySecretSource;
use crate::config::NetworkProxyConfig;
use crate::mitm_hook::MitmHookActionsConfig;
use crate::mitm_hook::MitmHookConfig;
use crate::mitm_hook::MitmHookMatchConfig;
use crate::runtime::network_proxy_state_for_policy;
use pretty_assertions::assert_eq;
use rama_tls_rustls::dep::pki_types::PrivateKeyDer;
use rama_tls_rustls::dep::pki_types::pem::PemObject;
use rama_tls_rustls::dep::rcgen::CertificateParams;
use rama_tls_rustls::dep::rcgen::KeyPair;
use rama_tls_rustls::dep::tokio_rustls::TlsAcceptor;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::time::timeout;

const FORWARD_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

async fn tls_origin_with_hook(
    dir: &Path,
    alpn: &[u8],
    configure: impl FnOnce(&mut MitmHookConfig, u16),
) -> (MitmRequestContext, TcpListener, TlsAcceptor) {
    ensure_rustls_crypto_provider();
    let key = KeyPair::generate().expect("generate origin key");
    let cert = CertificateParams::new(vec!["localhost".to_string()])
        .expect("origin certificate parameters")
        .self_signed(&key)
        .expect("origin certificate");
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(cert.der().clone())
        .expect("trust origin certificate");
    let key =
        PrivateKeyDer::from_pem_slice(key.serialize_pem().as_bytes()).expect("parse origin key");
    let mut server_config =
        rustls::ServerConfig::builder_with_protocol_versions(rustls::ALL_VERSIONS)
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], key)
            .expect("origin TLS configuration");
    server_config.alpn_protocols = vec![alpn.to_vec()];
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind TLS origin");
    let port = listener.local_addr().expect("origin address").port();

    let mut config = NetworkProxyConfig {
        mitm: true,
        mode: NetworkMode::Full,
        allow_local_binding: Some(true),
        mitm_hooks: vec![MitmHookConfig {
            host: "localhost".to_string(),
            matcher: MitmHookMatchConfig {
                schemes: vec!["https".to_string()],
                ports: vec![port],
                methods: vec!["GET".to_string(), "POST".to_string()],
                path_prefixes: vec!["/".to_string()],
                ..Default::default()
            },
            actions: MitmHookActionsConfig::default(),
        }],
        ..Default::default()
    };
    configure(&mut config.mitm_hooks[0], port);
    config.set_credential_broker_enabled(/*enabled*/ true);
    config.set_allowed_domains(vec!["localhost".to_string()]);
    let app_state = Arc::new(network_proxy_state_for_policy(config));
    let roots = Arc::new(roots);
    let upstream =
        UpstreamClient::direct_with_tls_root_store(Arc::clone(&app_state), Arc::clone(&roots));
    let mitm = Arc::new(MitmState {
        ca: ManagedMitmCa::for_test(dir).expect("create test MITM CA"),
        allow_upstream_proxy: false,
        upstream_tls_root_store: roots,
        inspect: false,
        max_body_bytes: MITM_MAX_BODY_BYTES,
    });
    let context = MitmRequestContext {
        dial_policy: None,
        policy: MitmPolicyContext {
            target_host: "localhost".to_string(),
            target_port: port,
            scheme: Scheme::HTTPS,
            mode: NetworkMode::Full,
            app_state,
        },
        mitm,
        upstream,
    };
    (
        context,
        listener,
        TlsAcceptor::from(Arc::new(server_config)),
    )
}

struct ReceivedRequest {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn read_request(stream: &mut (impl AsyncRead + Unpin)) -> ReceivedRequest {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 16 * 1024, "bounded origin request headers");
        head.push(stream.read_u8().await.expect("read request headers"));
    }
    let head = String::from_utf8(head).expect("ASCII request headers");
    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .expect("request line")
        .split_whitespace()
        .nth(1)
        .expect("request target")
        .to_string();
    let headers = lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (name, value) = line.split_once(':').expect("header separator");
            (name.to_ascii_lowercase(), value.trim().to_string())
        })
        .collect::<HashMap<_, _>>();
    assert!(!headers.contains_key("transfer-encoding"));
    let length = headers
        .get("content-length")
        .map_or(0, |length| length.parse::<usize>().expect("content length"));
    assert!(length <= 1024, "bounded origin request body");
    let mut body = vec![0; length];
    stream
        .read_exact(&mut body)
        .await
        .expect("drain request body");
    ReceivedRequest {
        path,
        headers,
        body,
    }
}

#[tokio::test]
async fn forwards_substituted_body_over_tls() {
    let marker = b"codex-secret-0123456789abcdef";
    let dir = tempdir().expect("TLS fixture directory");
    let secret = dir.path().join("body-secret");
    std::fs::write(&secret, "expanded-payload").unwrap();
    let configure = |hook: &mut MitmHookConfig, port| {
        hook.matcher.path_prefixes = vec!["/allowed/".into()];
        hook.actions.strip_request_headers.push("digest".into());
        hook.actions.body_secret_substitution = Some(crate::BodySecretSubstitutionConfig {
            replacements: vec![crate::BodySecretReplacement {
                placeholder: String::from_utf8(marker.to_vec()).unwrap(),
                secret: ProxySecretSource::File(secret.display().to_string()),
            }],
            port,
            max_body_bytes: 64,
        });
    };
    let (context, listener, acceptor) =
        tls_origin_with_hook(dir.path(), b"http/1.1", configure).await;
    let context = Arc::new(context);
    let make_request = |path: &str| {
        let mut req = Request::builder()
            .method("POST")
            .uri(path)
            .header(HOST, format!("localhost:{}", context.policy.target_port))
            .body(Body::from(marker.to_vec()))
            .unwrap();
        *req.version_mut() = rama_http::Version::HTTP_2;
        let headers = req.headers_mut();
        headers.insert("content-length", HeaderValue::from(marker.len()));
        req
    };
    let denied = handle_mitm_request(make_request("/denied"), context.clone())
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let mut invalid = make_request("/allowed/item");
    invalid
        .headers_mut()
        .insert("digest", HeaderValue::from_static("sha-256=guest"));
    let denied = handle_mitm_request(invalid, context.clone()).await.unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let origin = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = acceptor.accept(socket).await.unwrap();
        let wire = read_request(&mut stream).await;
        stream.write_all(RESPONSE).await.unwrap();
        wire
    });
    let req = make_request("/allowed/item");
    let response = timeout(FORWARD_TIMEOUT, handle_mitm_request(req, context))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let wire = origin.await.unwrap();
    assert_eq!(wire.body.as_slice(), b"expanded-payload");
    assert_eq!(wire.path, "/allowed/item");
    assert_eq!(wire.headers["content-length"], wire.body.len().to_string());
}
