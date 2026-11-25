import type { TFunction } from "i18next";

type ErrorTranslationKey =
  | "errors.openRepoBeforePairing"
  | "errors.chooseRoleBeforePairing"
  | "errors.openRepoBeforeTask"
  | "errors.createFirstCommit"
  | "errors.approvalChanged"
  | "errors.taskNotAwaitingApproval"
  | "errors.manifestAlreadyExists"
  | "errors.openRepoFirst"
  | "errors.repoPathEmpty"
  | "errors.runtimeShuttingDown"
  | "errors.taskNotCancellable"
  | "errors.taskNotFound"
  | "errors.peerRevoked"
  | "errors.peerNotFound"
  | "errors.emptyDeviceLabel"
  | "errors.localNodeTicket"
  | "errors.mcpPreviewChanged"
  | "errors.switchModeToPair"
  | "errors.notGitRepo"
  | "errors.savedAddressDifferentDevice"
  | "errors.emptyMessage"
  | "errors.invalidLlmBaseUrl";

const KNOWN_ERROR_KEYS: Readonly<Record<string, ErrorTranslationKey>> = {
  "Open the same Git repository on both devices before pairing.": "errors.openRepoBeforePairing",
  "Choose a device role before pairing": "errors.chooseRoleBeforePairing",
  "Open a Git repository before running a task.": "errors.openRepoBeforeTask",
  "Create the first Git commit before running a task.": "errors.createFirstCommit",
  "The suggested fix has changed. Review the current fix before continuing.":
    "errors.approvalChanged",
  "This task is not awaiting approval.": "errors.taskNotAwaitingApproval",
  "Open a local Git repository first.": "errors.openRepoFirst",
  "Repository path cannot be empty.": "errors.repoPathEmpty",
  "Runtime is shutting down": "errors.runtimeShuttingDown",
  "This task can no longer be cancelled.": "errors.taskNotCancellable",
  "device label cannot be empty": "errors.emptyDeviceLabel",
  "This ticket belongs to the local node": "errors.localNodeTicket",
  "The project MCP configuration has changed. Refresh the preview before writing.":
    "errors.mcpPreviewChanged",
  "Switch to Client or Host mode to pair with another machine": "errors.switchModeToPair",
  "Selected folder is not a Git repository (missing .git). Run `git init` first.":
    "errors.notGitRepo",
  "Saved address belongs to a different device": "errors.savedAddressDifferentDevice",
  "Message cannot be empty": "errors.emptyMessage",
  "Message cannot be empty.": "errors.emptyMessage",
  "expected an HTTP(S) API base URL": "errors.invalidLlmBaseUrl",
};

const RELAY_ERROR_PREFIX_RE =
  /^(?:(?:git command failed|package manager \([^)]+\) error|transport error|storage error|serialization error|toml parse error|io error|llm client error|internal error):\s*)+/i;

const FOLDER_NOT_FOUND_RE = /^Folder does not exist:\s*(.+)$/i;
const NOT_A_DIRECTORY_RE = /^(?:Repository path|RELAY_REPO_ROOT) is not a directory:\s*(.+)$/i;
const UNKNOWN_PRESET_RE =
  /^invalid command preset '([^']+)'(?:\s*\(not declared in relay\.toml\))?$/i;
const INVALID_MCP_JSON_RE = /^Refusing to overwrite ([^:]+): existing file contains invalid JSON/i;
const INVALID_MCP_STRUCTURE_RE =
  /^Refusing to overwrite ([^:]+): (?:root JSON value|"mcpServers") must be (?:an|a JSON) object\.?$/i;

const SUMMARY_MISSING_DEP_RE = /^(?:Smoke preflight|Quick check) failed: missing dependency (.+)$/i;
const SUMMARY_FAILED_EXIT_CODE_RE =
  /^(?:Smoke preflight|Quick check) failed \(exit code (?:Some\((\d+)\)|(\d+)|None)\)$/i;
const SUMMARY_CANCELLED_PARTIAL_RE = /^Cancelled by user \(partial log [^)]+\)$/i;

const PROBE_MODEL_AVAILABLE_RE = /^Connected \((\d+) ms\) — model '([^']+)' available$/i;
const PROBE_VERIFY_MODEL_RE =
  /^Connected \((\d+) ms\) — endpoint reachable \(verify model '([^']+)'\)$/i;
const PROBE_VIA_ENDPOINT_RE = /^Connected \((\d+) ms\) via (.+)$/i;
const PROBE_HTTP_ERROR_RE = /^HTTP (.+?) from (.+)$/i;
const PROBE_PREVIEW_OK_RE = /^Preview OK \((\d+) ms\)/i;

const LLM_CONFIG_ERROR_RE =
  /\b(?:http\s*\d{3}|401|403|404|429|5\d{2}|api[\s_-]?key|unauthorized|forbidden|authentication|model|ollama|openai|deepseek|base\s*url|endpoint|connect|connection|refused|econnrefused|unreachable|network|socket|timeout|timed\s*out|dns|tls|ssl|llm|provider|rate\s*limit)\b|接口|模型|连通|超时|连接|密钥|鉴权|网络/i;

function extractRawMessage(err: unknown): string {
  const raw =
    err instanceof Error
      ? err.message
      : typeof err === "string"
        ? err
        : err == null
          ? ""
          : String(err);
  return raw
    .trim()
    .replace(/^Error:\s*/i, "")
    .trim();
}

function stripRelayPrefixes(message: string): string {
  return message.replace(RELAY_ERROR_PREFIX_RE, "").trim();
}

function translateKnownError(normalized: string, t: TFunction): string | null {
  if (Object.hasOwn(KNOWN_ERROR_KEYS, normalized)) {
    return t(KNOWN_ERROR_KEYS[normalized]);
  }
  const lower = normalized.toLowerCase();
  if (lower.includes("relay.toml already exists")) {
    return t("errors.manifestAlreadyExists");
  }
  if (lower.includes("not a git repository (missing .git)")) {
    return t("errors.notGitRepo");
  }
  if (normalized.includes("invalid state transition") && lower.includes("cancelled")) {
    return t("errors.taskNotCancellable");
  }
  if (lower.startsWith("task not found:")) {
    return t("errors.taskNotFound");
  }
  if (lower.includes("peer_revoked") || lower.includes("peer has been revoked")) {
    return t("errors.peerRevoked");
  }
  if (lower.includes("trusted peer not found")) {
    return t("errors.peerNotFound");
  }
  if (lower.includes("mcp_preview_changed")) {
    return t("errors.mcpPreviewChanged");
  }
  if (lower.includes("expected an http(s) api base url")) {
    return t("errors.invalidLlmBaseUrl");
  }

  const folderMatch = FOLDER_NOT_FOUND_RE.exec(normalized);
  if (folderMatch) {
    return t("errors.folderNotFound", { path: folderMatch[1].trim() });
  }
  const notDirMatch = NOT_A_DIRECTORY_RE.exec(normalized);
  if (notDirMatch) {
    return t("errors.pathNotDirectory", { path: notDirMatch[1].trim() });
  }
  const presetMatch = UNKNOWN_PRESET_RE.exec(normalized);
  if (presetMatch) {
    return t("errors.unknownPreset", { preset: presetMatch[1] });
  }
  const mcpJsonMatch = INVALID_MCP_JSON_RE.exec(normalized);
  if (mcpJsonMatch) {
    return t("errors.invalidMcpJson", { file: mcpJsonMatch[1].trim() });
  }
  const mcpStructMatch = INVALID_MCP_STRUCTURE_RE.exec(normalized);
  if (mcpStructMatch) {
    return t("errors.invalidMcpJsonStructure", { file: mcpStructMatch[1].trim() });
  }

  return null;
}

export function formatErrorMessage(err: unknown, t: TFunction, fallback?: string): string {
  const cleaned = extractRawMessage(err);
  if (!cleaned) {
    return fallback ?? t("errors.generic");
  }
  const normalized = stripRelayPrefixes(cleaned);
  if (!normalized) {
    return fallback ?? t("errors.generic");
  }
  const translated = translateKnownError(normalized, t);
  return translated ?? normalized;
}

export function isMcpPreviewChangedError(err: unknown): boolean {
  const normalized = stripRelayPrefixes(extractRawMessage(err));
  const lower = normalized.toLowerCase();
  return (
    normalized ===
      "The project MCP configuration has changed. Refresh the preview before writing." ||
    lower.includes("mcp_preview_changed")
  );
}

export function isLlmConfigurationError(message: string | null | undefined): boolean {
  if (!message) return false;
  return LLM_CONFIG_ERROR_RE.test(message);
}

export function formatTaskSummary(summary: string, t: TFunction): string {
  const trimmed = summary.trim();
  if (!trimmed) return "";

  const lower = trimmed.toLowerCase();
  if (lower === "smoke preflight passed" || lower === "quick check passed") {
    return t("taskSummary.quickCheckPassed");
  }
  if (lower === "smoke preflight timed out" || lower === "quick check timed out") {
    return t("taskSummary.quickCheckTimedOut");
  }
  if (lower === "cancelled by user") {
    return t("taskSummary.cancelledByUser");
  }
  if (SUMMARY_CANCELLED_PARTIAL_RE.test(trimmed)) {
    return t("taskSummary.cancelledWithPartialLog");
  }
  if (lower === "waiting for host pre-check" || lower === "waiting for preflight") {
    return t("taskSummary.waitingForHostCheck");
  }
  if (lower === "fix applied and run finished cleanly.") {
    return t("taskSummary.fixAppliedFinished");
  }
  if (lower === "suggested fix dismissed.") {
    return t("taskSummary.fixDismissed");
  }
  if (lower === "quick check failed (terminated by signal)") {
    return t("taskSummary.quickCheckFailedSignal");
  }

  const missingDepMatch = SUMMARY_MISSING_DEP_RE.exec(trimmed);
  if (missingDepMatch) {
    return t("taskSummary.quickCheckMissingDep", { module: missingDepMatch[1].trim() });
  }

  const exitCodeMatch = SUMMARY_FAILED_EXIT_CODE_RE.exec(trimmed);
  if (exitCodeMatch) {
    const code = exitCodeMatch[1] ?? exitCodeMatch[2];
    if (code) {
      return t("taskSummary.quickCheckFailedCode", { code });
    }
    return t("taskSummary.quickCheckFailed");
  }

  if (lower === "smoke preflight failed" || lower === "quick check failed") {
    return t("taskSummary.quickCheckFailed");
  }

  return trimmed;
}

export function formatLlmProbeMessage(message: string, t: TFunction): string {
  const trimmed = message.trim();
  if (!trimmed) {
    return t("modals.workspace.errors.testLlm");
  }

  const availMatch = PROBE_MODEL_AVAILABLE_RE.exec(trimmed);
  if (availMatch) {
    return t("modals.workspace.probe.connectedModelAvailable", {
      ms: availMatch[1],
      model: availMatch[2],
    });
  }

  const verifyMatch = PROBE_VERIFY_MODEL_RE.exec(trimmed);
  if (verifyMatch) {
    return t("modals.workspace.probe.connectedVerifyModel", {
      ms: verifyMatch[1],
      model: verifyMatch[2],
    });
  }

  const viaMatch = PROBE_VIA_ENDPOINT_RE.exec(trimmed);
  if (viaMatch) {
    return t("modals.workspace.probe.connectedViaEndpoint", {
      ms: viaMatch[1],
      endpoint: viaMatch[2].trim(),
    });
  }

  const httpMatch = PROBE_HTTP_ERROR_RE.exec(trimmed);
  if (httpMatch) {
    return t("modals.workspace.probe.httpError", {
      status: httpMatch[1].trim(),
      endpoint: httpMatch[2].trim(),
    });
  }

  const previewMatch = PROBE_PREVIEW_OK_RE.exec(trimmed);
  if (previewMatch) {
    return t("modals.workspace.probe.previewOk", { ms: previewMatch[1] });
  }

  return formatErrorMessage(trimmed, t, t("modals.workspace.errors.testLlm"));
}

export function formatDeviceOrigin(origin: string, t: TFunction): string {
  const trimmed = origin.trim();
  const lower = trimmed.toLowerCase();
  if (lower === "relay-copilot" || lower === "synceverything agent") {
    return t("copilot.emptyTitle");
  }
  if (lower === "workstation" || lower === "host") {
    return t("sidebar.mode.host");
  }
  if (lower === "laptop" || lower === "client") {
    return t("sidebar.mode.client");
  }
  if (lower === "local_dual_sandbox") {
    return t("sidebar.mode.local_dual_sandbox");
  }
  return trimmed;
}
