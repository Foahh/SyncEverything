use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout, Command};
use uuid::Uuid;

async fn git(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        command.env_remove(key);
    }
    let output = command
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn send(stdin: &mut ChildStdin, message: Value) {
    stdin
        .write_all(format!("{message}\n").as_bytes())
        .await
        .unwrap();
}

async fn response(lines: &mut Lines<BufReader<ChildStdout>>, id: u32) -> Value {
    loop {
        let line = lines.next_line().await.unwrap().expect("MCP server closed");
        let message: Value = serde_json::from_str(&line).unwrap();
        if message["id"] == id {
            assert!(message.get("error").is_none(), "{message}");
            return message["result"].clone();
        }
    }
}

#[tokio::test]
async fn open_mcp_session_tracks_new_git_head() {
    let temp = std::env::temp_dir().join(format!("relay-daemon-head-{}", Uuid::now_v7()));
    let repo = temp.join("repo");
    let data = temp.join("data");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init"]).await;
    std::fs::write(
        repo.join("relay.toml"),
        "[project]\nname='Test'\nrepo_id='test'\n",
    )
    .unwrap();
    git(&repo, &["add", "relay.toml"]).await;
    git(&repo, &["commit", "-m", "Initial"]).await;
    let initial = git(&repo, &["rev-parse", "HEAD"]).await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_relay-daemon"))
        .arg("--repo-root")
        .arg(&repo)
        .arg("--data-dir")
        .arg(data)
        .arg("mcp-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let check = tokio::time::timeout(Duration::from_secs(20), async {
        send(
            &mut stdin,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                           "clientInfo": {"name": "head-test", "version": "1"}}
            }),
        )
        .await;
        response(&mut lines, 1).await;
        send(
            &mut stdin,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;

        let mut expected = initial.clone();
        for id in 2.. {
            send(
                &mut stdin,
                json!({
                    "jsonrpc": "2.0", "id": id, "method": "tools/call",
                    "params": {"name": "get_dag_snapshot", "arguments": {}}
                }),
            )
            .await;
            let result = response(&mut lines, id).await;
            let snapshot: Value =
                serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
            if id == 2 {
                assert_eq!(snapshot["head_commit_sha"], initial);
                git(&repo, &["commit", "--allow-empty", "-m", "Advance HEAD"]).await;
                expected = git(&repo, &["rev-parse", "HEAD"]).await;
            } else if snapshot["head_commit_sha"] == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    })
    .await;
    child.kill().await.unwrap();
    std::fs::remove_dir_all(temp).unwrap();
    check.expect("MCP session did not observe the new Git HEAD");
}
