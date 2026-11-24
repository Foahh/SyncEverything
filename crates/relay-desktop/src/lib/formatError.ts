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
  | "errors.switchModeToPair";

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
  "Switch to Client or Host mode to pair with another machine": "errors.switchModeToPair",
};

const RELAY_ERROR_PREFIX_RE =
  /^(?:git command failed|transport error|internal error|uv command failed|llm client error):\s*/i;

function matchKnownErrorKey(message: string): ErrorTranslationKey | null {
  const normalized = message.replace(RELAY_ERROR_PREFIX_RE, "").trim();
  if (Object.hasOwn(KNOWN_ERROR_KEYS, normalized)) {
    return KNOWN_ERROR_KEYS[normalized];
  }
  const lower = normalized.toLowerCase();
  if (lower.includes("relay.toml already exists")) {
    return "errors.manifestAlreadyExists";
  }
  if (normalized.includes("invalid state transition") && lower.includes("cancelled")) {
    return "errors.taskNotCancellable";
  }
  if (lower.startsWith("task not found:")) {
    return "errors.taskNotFound";
  }
  if (lower.includes("peer_revoked") || lower.includes("peer has been revoked")) {
    return "errors.peerRevoked";
  }
  if (lower.includes("trusted peer not found")) {
    return "errors.peerNotFound";
  }
  return null;
}

export function formatErrorMessage(err: unknown, t: TFunction, fallback?: string): string {
  const raw =
    err instanceof Error
      ? err.message
      : typeof err === "string"
        ? err
        : err == null
          ? ""
          : String(err);
  const cleaned = raw
    .trim()
    .replace(/^Error:\s*/i, "")
    .trim();
  if (!cleaned) {
    return fallback ?? t("errors.generic");
  }
  const knownKey = matchKnownErrorKey(cleaned);
  return knownKey ? t(knownKey) : cleaned;
}
