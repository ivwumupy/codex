//! Header authentication propagates actor and account identity over both transports.

use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_login::AuthHeaders;
use codex_login::CodexAuth;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use http::HeaderMap;
use http::HeaderValue;
use pretty_assertions::assert_eq;

const ACTOR: &str = "x-openai-actor-authorization";
const ACCOUNT: &str = "chatgpt-account-id";

fn actor_headers() -> HeaderMap {
    HeaderMap::from_iter([
        (
            http::HeaderName::from_static(ACTOR),
            HeaderValue::from_static("verified-actor"),
        ),
        (
            http::HeaderName::from_static(ACCOUNT),
            HeaderValue::from_static("workspace-1"),
        ),
    ])
}

fn actor_auth() -> CodexAuth {
    CodexAuth::Headers(AuthHeaders::new(actor_headers()))
}

async fn submit(test: &TestCodex) -> Result<()> {
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "hello".to_owned(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let event = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_) | EventMsg::Error(_))
    })
    .await;
    if let EventMsg::Error(error) = event {
        anyhow::bail!("actor request failed: {error:?}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_http_preserves_identity() -> Result<()> {
    core_test_support::skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_sequence(
        &server,
        (0..2)
            .map(|i| responses::sse_completed(&format!("resp-{i}")))
            .collect(),
    )
    .await;
    let test = test_codex()
        .with_auth(actor_auth())
        .with_config(|config| {
            config.model_provider_id = "local".to_owned();
        })
        .build_with_auto_env(&server)
        .await?;
    for _ in 0..2 {
        submit(&test).await?;
    }
    assert_eq!(
        requests
            .requests()
            .iter()
            .map(|request| (request.header(ACTOR), request.header(ACCOUNT),))
            .collect::<Vec<_>>(),
        vec![
            (
                Some("verified-actor".to_owned()),
                Some("workspace-1".to_owned())
            );
            2
        ]
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_websocket_reuse_preserves_identity() -> Result<()> {
    core_test_support::skip_if_no_network!(Ok(()));
    let server = responses::start_websocket_server(vec![
        (0..3)
            .map(|i| vec![responses::ev_completed(&format!("resp-{i}"))])
            .collect(),
    ])
    .await;
    let base_url = format!("{}/v1", server.uri().replacen("ws://", "http://", 1));
    let test = test_codex()
        .with_auth(actor_auth())
        .with_config(move |config| {
            config.model_provider_id = "local".to_owned();
            config.model_provider.base_url = Some(base_url);
        })
        .build_with_websocket_server(&server)
        .await?;
    for _ in 0..2 {
        submit(&test).await?;
    }
    assert_eq!(server.single_connection().len(), 3);
    let handshakes = server.handshakes();
    assert_eq!(
        handshakes
            .iter()
            .map(|handshake| (handshake.header(ACTOR), handshake.header(ACCOUNT)))
            .collect::<Vec<_>>(),
        vec![(
            Some("verified-actor".to_owned()),
            Some("workspace-1".to_owned())
        )]
    );
    test.codex.shutdown_and_wait().await?;
    server.shutdown().await;
    Ok(())
}
