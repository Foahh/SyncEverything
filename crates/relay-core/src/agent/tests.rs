use super::*;
use crate::contracts::GraphNode;
use chrono::Utc;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket};
use tokio::task::JoinHandle;
use uuid::Uuid;

fn failure(module: &str) -> PreflightReport {
    let traceback = format!("ModuleNotFoundError: No module named '{module}'");
    PreflightReport {
        task_id: Uuid::now_v7(),
        worktree_path: PathBuf::from("isolated"),
        worktree_branch: "relay/worktree/test".into(),
        smoke_passed: false,
        exit_code: Some(1),
        stdout_tail: String::new(),
        stderr_tail: traceback.clone(),
        detected_missing_module: Some(module.into()),
        raw_log: traceback,
    }
}

fn endpoint(base_url: String) -> LlmClient {
    LlmClient::new(
        LlmEndpointConfig { base_url, ..LlmEndpointConfig::default() },
        Duration::from_secs(5),
    )
    .unwrap()
}

fn evidence(id: &str, freshness: Freshness, metric: &str) -> GraphNode {
    let now = Utc::now();
    GraphNode {
        id: id.into(),
        label: format!("{id} label"),
        payload: NodeKind::Evidence {
            task_id: "task".into(),
            preset: "train_grid".into(),
            metrics_json: json!({"metric": metric}).into(),
            log_blob_hash: "log-hash".into(),
        },
        freshness,
        device_origin: "local".into(),
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn agent_safe_dag_context_quarantines_stale_evidence() {
    let now = Utc::now();
    let snapshot = DagSnapshot {
        repo_id: "test".into(),
        head_commit_sha: None,
        nodes: vec![
            evidence("fresh-id", Freshness::Fresh, "fresh-score"),
            evidence("pending-id", Freshness::PendingVerify, "pending-score"),
            evidence(
                "stale-id",
                Freshness::Stale {
                    caused_by: "new-artifact".into(),
                    reason: "superseded".into(),
                    marked_at: now,
                },
                "quarantined-score",
            ),
        ],
        edges: vec![],
        stale_count: 1,
        captured_at: now,
    };
    let context = build_safe_dag_context(&snapshot);
    let (eligible, quarantine) = context
        .split_once("[QUARANTINED STALE EVIDENCE \u{2014} DO NOT CITE]\n")
        .unwrap();
    assert!(eligible.contains("fresh-score"));
    assert!(eligible.contains("pending-score"));
    assert!(!eligible.contains("stale-id"));
    assert_eq!(quarantine, "\"stale-id\"\n");
    assert!(!context.contains("quarantined-score"));
}

#[tokio::test]
async fn agent_deterministic_fallback_diagnoses_missing_module() {
    // Keep this socket bound without listening so no other test can claim the port.
    let socket = TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let client = endpoint(format!("http://{}/v1", socket.local_addr().unwrap()));
    let agent = SelfHealingAgent::new(client);
    for (module, package) in [("scipy", "scipy"), ("sklearn.model_selection", "scikit-learn")] {
        let proposal = agent.diagnose_preflight_failure(&failure(module), "").await.unwrap();
        assert_eq!(proposal.patch_kind, PatchKind::UvSpec);
        assert_eq!(proposal.content, package);
        assert_eq!(proposal.source, "deterministic_fallback");
    }

    let mut report = failure("scipy");
    report.detected_missing_module = None;
    assert!(matches!(agent.diagnose_preflight_failure(&report, "").await, Err(RelayError::Llm(_))));
}

async fn mock_completion(content: String) -> (String, JoinHandle<serde_json::Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        let body_start = loop {
            let count = socket.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&chunk[..count]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let headers = std::str::from_utf8(&request[..body_start]).unwrap();
        assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"), "{headers}");
        let length: usize = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .unwrap()
            .1
            .trim()
            .parse()
            .unwrap();
        while request.len() < body_start + length {
            let count = socket.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&chunk[..count]);
        }
        let body = serde_json::from_slice(&request[body_start..body_start + length]).unwrap();
        let response = json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let http = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
            response.len(),
        );
        socket.write_all(http.as_bytes()).await.unwrap();
        body
    });
    (format!("http://{addr}/v1/"), server)
}

#[tokio::test]
async fn llm_client_parses_openai_chat_completions() {
    let (url, server) = mock_completion("hello from Relay".into()).await;
    let messages = [ChatMessage { role: "user".into(), content: "hello".into() }];
    let reply = endpoint(url).chat_completion(&messages).await.unwrap();
    assert_eq!(reply.choices[0].message.content.as_deref(), Some("hello from Relay"));
    let body = server.await.unwrap();
    assert_eq!(body["model"], "qwen3.5:4b");
    assert_eq!(body["messages"], json!(messages));
    assert_eq!(body["stream"], false);
}

#[tokio::test]
async fn agent_uses_structured_llm_diagnosis() {
    let response = json!({
        "patch_kind": "uv_spec",
        "summary": "Add the missing library",
        "content": "scipy>=1.14"
    });
    let (url, server) = mock_completion(response.to_string()).await;
    let proposal = SelfHealingAgent::new(endpoint(url))
        .diagnose_preflight_failure(&failure("scipy"), "Forecast fixture")
        .await
        .unwrap();
    assert_eq!(proposal.patch_kind, PatchKind::UvSpec);
    assert_eq!(proposal.content, "scipy>=1.14");
    assert_eq!(proposal.source, "llm");
    let body = server.await.unwrap();
    assert_eq!(body["response_format"]["type"], "json_object");
    assert!(body["messages"][1]["content"].as_str().unwrap().contains("Forecast fixture"));
}
