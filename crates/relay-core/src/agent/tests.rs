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
        error_snippets: Vec::new(),
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
        assert_eq!(proposal.uv_spec.as_deref(), Some(package));
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
        "uv_spec": "scipy>=1.14"
    });
    let (url, server) = mock_completion(response.to_string()).await;
    let proposal = SelfHealingAgent::new(endpoint(url))
        .diagnose_preflight_failure(&failure("scipy"), "Forecast fixture")
        .await
        .unwrap();
    assert_eq!(proposal.patch_kind, PatchKind::UvSpec);
    assert_eq!(proposal.uv_spec.as_deref(), Some("scipy>=1.14"));
    assert_eq!(proposal.source, "llm");
    let body = server.await.unwrap();
    assert_eq!(body["response_format"]["type"], "json_object");
    assert!(body["messages"][1]["content"].as_str().unwrap().contains("Forecast fixture"));
}

#[tokio::test]
async fn agent_parses_markdown_fenced_json_diagnosis() {
    let fenced = "```json\n{\"patch_kind\":\"file_edits\",\"summary\":\"Define constant\",\"edits\":[{\"file_path\":\"src/app.py\",\"old_text\":\"a = 1\",\"new_text\":\"a = 2\"}]}\n```";
    let (url, server) = mock_completion(fenced.into()).await;
    let proposal = SelfHealingAgent::new(endpoint(url))
        .diagnose_preflight_failure(&failure("scipy"), "Fixture")
        .await
        .unwrap();
    assert_eq!(proposal.patch_kind, PatchKind::FileEdits);
    assert_eq!(proposal.edits.as_ref().unwrap().len(), 1);
    let _ = server.await.unwrap();
}

#[tokio::test]
async fn llm_client_probe_and_config_persistence_work() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let count = socket.read(&mut buf).await.unwrap();
        let req = std::str::from_utf8(&buf[..count]).unwrap().to_owned();
        let body = r#"{"data":[{"id":"qwen3.5:9b"}]}"#;
        let http = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len(),
        );
        socket.write_all(http.as_bytes()).await.unwrap();
        req
    });

    let cfg = LlmEndpointConfig {
        tier: InferenceTier::CloudOpenAiCompat,
        base_url: format!("http://{addr}/v1"),
        model: "qwen3.5:9b".into(),
        api_key_env: None,
        api_key: Some("sk-test-secret-1234".into()),
    };
    let view = cfg.redacted_view();
    assert!(view.has_api_key);
    assert_eq!(view.api_key_hint.as_deref(), Some("sk-••••1234"));

    let tmp_path = std::env::temp_dir()
        .join(format!("relay-llm-cfg-{}", Uuid::now_v7()))
        .join("llm-settings.json");
    save_llm_config(&tmp_path, &cfg).unwrap();
    let loaded = load_llm_config(&tmp_path).unwrap();
    assert_eq!(loaded, cfg);
    let _ = std::fs::remove_dir_all(tmp_path.parent().unwrap());

    let client = LlmClient::new(loaded, Duration::from_secs(5)).unwrap();
    let probe = client.probe().await;
    assert!(probe.ok, "probe failed: {}", probe.message);
    assert!(probe.message.contains("qwen3.5:9b"));
    let req = server.await.unwrap();
    assert!(req.starts_with("GET /v1/models HTTP/1.1\r\n"), "{req}");
    assert!(
        req.to_ascii_lowercase().contains("authorization: bearer sk-test-secret-1234"),
        "{req}"
    );
}

#[test]
fn compose_context_ablation_strategies_are_deterministic_and_quarantine_stale_in_dag_bundle() {
    use crate::contracts::{EdgeKind, GraphEdge, TaskSpec};
    use std::collections::BTreeMap;

    let now = Utc::now();
    let snapshot = DagSnapshot {
        repo_id: "forecast-bench".into(),
        head_commit_sha: Some("deadbeef1234567".into()),
        nodes: vec![
            GraphNode {
                id: "dec-1".into(),
                label: "Blend alpha=0.45".into(),
                payload: NodeKind::Decision {
                    rationale: "Improve seasonal tracking".into(),
                    constraints: vec!["Keep smoke < 2s".into()],
                },
                freshness: Freshness::Fresh,
                device_origin: "laptop".into(),
                created_at: now,
                updated_at: now,
            },
            evidence("ev-fresh", Freshness::Fresh, "1.942-fresh"),
            evidence("ev-pending", Freshness::PendingVerify, "unverified"),
            evidence(
                "ev-stale",
                Freshness::Stale {
                    caused_by: "art-v2".into(),
                    reason: "Upstream edit".into(),
                    marked_at: now,
                },
                "2.165-stale-secret",
            ),
        ],
        edges: vec![GraphEdge {
            id: "e1".into(),
            from: "dec-1".into(),
            to: "ev-fresh".into(),
            kind: EdgeKind::Informs,
        }],
        stale_count: 1,
        captured_at: now,
    };
    let mut tasks = vec![TaskRecord {
        spec: TaskSpec {
            task_id: Uuid::now_v7(),
            repo_id: "forecast-bench".into(),
            base_commit_sha: "deadbeef1234567".into(),
            command_preset: "train_grid".into(),
            params: BTreeMap::new(),
            require_approval: true,
            requested_by_device: "laptop".into(),
            user_note: Some("Run grid".into()),
        },
        state: TaskState::Queued,
        active_proposal: None,
        preflight_summary: Some("Waiting for preflight".into()),
        evidence_node_id: None,
        error_message: None,
        created_at: now,
        updated_at: now,
    }];
    let mut failed = tasks[0].clone();
    failed.spec.task_id = Uuid::now_v7();
    failed.spec.command_preset = "failed-preset".into();
    failed.state = TaskState::Failed;
    tasks.push(failed);
    let handoffs = vec![HandoffSummary {
        handoff_id: "ho-1".into(),
        repo_id: "forecast-bench".into(),
        source_device: "laptop".into(),
        base_commit_sha: "deadbeef".into(),
        summary: "Updated blend weights".into(),
        active_task_count: 1,
        stale_evidence_count: 1,
        created_at: now,
    }];

    let full = compose_context(&snapshot, &tasks, &handoffs, ContextStrategy::FullTranscript);
    let flat = compose_context(&snapshot, &tasks, &handoffs, ContextStrategy::FlatSummary);
    let dag = compose_context(&snapshot, &tasks, &handoffs, ContextStrategy::DagBundle);
    let dag_repeat = compose_context(&snapshot, &tasks, &handoffs, ContextStrategy::DagBundle);

    assert_eq!(dag.context_bundle_hash, dag_repeat.context_bundle_hash);
    assert!(dag.context_bundle_hash.starts_with("blake3:"));
    assert_eq!(full.exposed_stale_metrics_count, 1);
    assert_eq!(flat.exposed_stale_metrics_count, 1);
    assert_eq!(dag.exposed_stale_metrics_count, 0);
    assert_eq!(dag.fresh_evidence_count, 1);
    assert!(!dag.content.contains("failed-preset"));

    assert!(full.content.contains("2.165-stale-secret"));
    assert!(flat.content.contains("2.165-stale-secret"));
    assert!(!dag.content.contains("2.165-stale-secret"));
    assert!(dag.content.contains("1.942-fresh"));
    assert!(dag.content.contains("\"ev-stale\""));
    assert!(dag.estimated_tokens < full.estimated_tokens);
}

#[tokio::test]
async fn stream_chat_with_tools_parses_chunked_sse_text_and_tool_calls() {
    use std::sync::atomic::AtomicBool;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 8192];
        let _ = socket.read(&mut buf).await.unwrap();
        socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n").await.unwrap();
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Checking \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"结果...\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_abc\",\"function\":{\"name\":\"get_dag_\",\"arguments\":\"{\\\"include_\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"snapshot\",\"arguments\":\"stale\\\":true}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let split = sse.find('结').unwrap() + 1;
        for chunk in [&sse.as_bytes()[..split], &sse.as_bytes()[split..]] {
            socket.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await.unwrap();
            socket.write_all(chunk).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
        }
        socket.write_all(b"0\r\n\r\n").await.unwrap();
    });

    let client = endpoint(format!("http://{addr}/v1"));
    let cancel = AtomicBool::new(false);
    let mut streamed = Vec::new();
    let turn = client
        .stream_chat_with_tools(
            &[json!({"role": "user", "content": "status?"})],
            &copilot_tool_definitions(),
            &cancel,
            |delta| streamed.push(delta.to_owned()),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(streamed, vec!["Checking ", "结果..."]);
    assert_eq!(turn.content, "Checking 结果...");
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].id, "call_abc");
    assert_eq!(turn.tool_calls[0].name, "get_dag_snapshot");
    assert_eq!(turn.tool_calls[0].arguments, "{\"include_stale\":true}");
}

#[tokio::test]
async fn stream_chat_with_tools_falls_back_to_non_streaming_on_http_400() {
    use std::sync::atomic::AtomicBool;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        // First request: reject streaming with HTTP 400
        {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf).await.unwrap();
            let resp = "HTTP/1.1 400 Bad Request\r\ncontent-length: 23\r\nconnection: close\r\n\r\nstreaming not supported";
            socket.write_all(resp.as_bytes()).await.unwrap();
        }
        // Second request: return non-streaming JSON
        {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf).await.unwrap();
            let body = json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "Fallback ok",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "inspect_sync_status",
                                "arguments": "{}"
                            }
                        }]
                    }
                }]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(resp.as_bytes()).await.unwrap();
        }
    });

    let client = endpoint(format!("http://{addr}/v1"));
    let cancel = AtomicBool::new(false);
    let mut streamed = String::new();
    let turn = client
        .stream_chat_with_tools(
            &[json!({"role": "user", "content": "sync?"})],
            &copilot_tool_definitions(),
            &cancel,
            |delta| streamed.push_str(delta),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(streamed, "Fallback ok");
    assert_eq!(turn.content, "Fallback ok");
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].name, "inspect_sync_status");
}

#[tokio::test]
async fn engine_persists_chat_history_and_resolves_approvals() {
    let engine = RelayEngine::in_memory("forecast-bench", None).await.unwrap();
    let now = Utc::now();
    let user_msg = AgentChatMessage {
        id: "msg-1".into(),
        repo_id: "forecast-bench".into(),
        role: AgentChatRole::User,
        content: "Run smoke check".into(),
        traces: vec![],
        approval: None,
        error: None,
        created_at: now,
    };
    let assistant_msg = AgentChatMessage {
        id: "msg-2".into(),
        repo_id: "forecast-bench".into(),
        role: AgentChatRole::Assistant,
        content: "Patch ready for review.".into(),
        traces: vec![AgentToolTrace {
            id: "tr-1".into(),
            tool_name: "propose_worktree_patch".into(),
            args_preview: "{\"task_id\":\"t1\"}".into(),
            status: ToolTraceStatus::Done,
            output_preview: Some("smoke_verified: true".into()),
            duration_ms: Some(120),
        }],
        approval: Some(AgentApprovalCard {
            task_id: "task-42".into(),
            patch_id: Some("patch-9".into()),
            summary: "Add scipy dependency".into(),
            diff_preview: "+scipy>=1.14".into(),
            resolved: None,
        }),
        error: None,
        created_at: now + chrono::Duration::milliseconds(5),
    };

    engine.upsert_chat_message(&user_msg).await.unwrap();
    engine.upsert_chat_message(&assistant_msg).await.unwrap();

    let listed = engine.list_chat_messages(50).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, "msg-1");
    assert_eq!(listed[1].traces.len(), 1);
    assert_eq!(listed[1].approval.as_ref().unwrap().resolved, None);

    for patch in [None, Some("previous-patch")] {
        assert_eq!(
            engine.mark_chat_approval_resolved("task-42", patch, "approved").await.unwrap(),
            0
        );
    }
    let updated = engine
        .mark_chat_approval_resolved("task-42", Some("patch-9"), "approved")
        .await
        .unwrap();
    assert_eq!(updated, 1);

    // A final stream update can arrive after the user has already acted on the card.
    engine.upsert_chat_message(&assistant_msg).await.unwrap();
    let after = engine.list_chat_messages(50).await.unwrap();
    assert_eq!(
        after[1].approval.as_ref().unwrap().resolved.as_deref(),
        Some("approved")
    );

    engine.clear_chat_messages().await.unwrap();
    assert!(engine.list_chat_messages(50).await.unwrap().is_empty());
}
