# Security Policy

## Supported Versions

| Version | Supported |
| --- | --- |
| `0.1.x` | Yes |

## Reporting a Vulnerability

Please **do not** report security vulnerabilities through public GitHub issues.

Instead, report vulnerabilities privately via [GitHub Security Advisories](https://github.com/Foahh/relay/security/advisories/new) on `https://github.com/Foahh/relay`. Include:

- A clear description of the vulnerability and its potential impact.
- Reproduction steps, affected component (`relay-core`, `relay-daemon`, `relay-desktop`, or MCP tools), and environment details.
- Any suggested fix or mitigation if known.

Maintainers aim to acknowledge reports within 72 hours and coordinate a fix before public disclosure.

---

## Security Model & Trust Boundaries

SyncEverything (powered by the Relay engine) coordinates Git state and task execution across developer machines. Understanding its security boundaries is essential before running a Host daemon.

### 1. `relay.toml` Preset Whitelist (No Arbitrary Shell Execution)

- The Host daemon **never executes arbitrary shell strings** received over P2P RPC or MCP.
- Every task must match a named `[presets.<name>]` entry in the target repository's `relay.toml` defining an explicit `program`, fixed `args`, `timeout_secs`, and optional `allowed_params`.
- Commands are spawned directly via `tokio::process::Command` without invoking a system shell (`sh -c` / `cmd /C`), eliminating shell metacharacter injection.
- Any task parameter key not listed in `allowed_params` is deterministically rejected before execution.

### 2. Isolated `git worktree` Boundaries

- Preflights, self-healing proposals (`uv_spec`, `file_edits`), and preset executions run inside per-task Git worktrees created from the requested `base_commit_sha`, leaving the user's main working checkout untouched.
- If a candidate self-healing patch fails smoke verification, tracked and untracked worktree modifications are automatically rolled back.
- The `read_worktree_file` MCP tool enforces strict canonical path confinement inside the task worktree and blocks `..` traversal, symlink escapes outside the worktree, binary files, and sensitive directories/files (`.git/`, `.venv/`, `.relay/`, `.env*`).

### 3. Local User Account Privilege Scope

- Git worktrees provide **checkout and file isolation, not OS-level sandboxing or containerization**.
- Preset processes run under the Host's local OS user account with that user's filesystem, network, and GPU permissions.
- Therefore, only run a Host against **trusted repositories** whose `relay.toml` presets and scripts you have reviewed. Do not point `relay-daemon` at untrusted third-party codebases without external OS/container isolation.

### 4. P2P Peer Trust & Revocation Model

- Peer-to-peer links run over encrypted QUIC via `iroh`, authenticating peers by Ed25519 `NodeId` and verifying synced blobs (Git bundles, diffs, execution logs) by BLAKE3 content hashes.
- Pairing relies on short-lived one-time pairing tickets (`--pairing-window-secs`, defaulting to 120 seconds in SyncEverything Desktop and 600 seconds in `relay-daemon serve`).
- Use `--lan-only` for direct private-LAN pairing without public relay servers, or `--local-only` for single-machine loopback testing.
- **Revoking a retired or compromised device**: Because interactive peer revocation in Desktop Settings is planned for a future release, revoke trust today by stopping the daemon/desktop app, removing the peer's persisted trust record or rotating the node's key material inside your local `--data-dir` (default `.relay/`), and restarting with a fresh pairing ticket.

### 5. Human-in-the-Loop Approval & Secret Hygiene

- Tasks configured with `require_approval = true` or carrying a self-healing patch cannot execute until confirmed via native MCP Elicitation or SyncEverything Desktop's Diff Approval flow (`ApprovalRequired` boundary).
- Store LLM API keys locally (via environment variables or local `.relay` settings) and never commit `.env`, `.relay/`, or `*.db` files. Loopback LLM URLs (`127.0.0.1` / `localhost` / `::1`) do not receive cloud keys from `.env` unless explicitly configured.
