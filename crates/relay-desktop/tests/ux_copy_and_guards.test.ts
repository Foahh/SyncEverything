import { describe, expect, test } from "bun:test";
import i18n from "../src/i18n";
import en from "../src/i18n/locales/en.json";
import zhCN from "../src/i18n/locales/zh-CN.json";
import {
  formatDeviceOrigin,
  formatErrorMessage,
  formatLlmProbeMessage,
  formatTaskSummary,
  isLlmConfigurationError,
  isMcpPreviewChangedError,
} from "../src/lib/formatError";
import type { McpHttpUrlErrorCode } from "../src/lib/mcpConfig";
import { isValidNoteLabel, parseNoteConstraints } from "../src/lib/noteForm";

function collectLeafKeysRecursive(obj: Record<string, unknown>, prefix = ""): string[] {
  const keys: string[] = [];
  for (const [key, value] of Object.entries(obj)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object" && !Array.isArray(value)) {
      keys.push(...collectLeafKeysRecursive(value as Record<string, unknown>, full));
    } else {
      keys.push(full);
    }
  }
  return keys.sort();
}

describe("note form helpers", () => {
  test("accepts concise 1-2 character Chinese and English note labels", () => {
    expect(isValidNoteLabel("提速")).toBe(true);
    expect(isValidNoteLabel("回滚")).toBe(true);
    expect(isValidNoteLabel("v2")).toBe(true);
    expect(isValidNoteLabel("a")).toBe(true);
    expect(isValidNoteLabel("  OK  ")).toBe(true);
    expect(isValidNoteLabel("")).toBe(false);
    expect(isValidNoteLabel("   \t\n ")).toBe(false);
  });

  test("splits constraints on both ASCII commas and Chinese full-width commas", () => {
    expect(parseNoteConstraints("keep runtime < 10s, lock deps")).toEqual([
      "keep runtime < 10s",
      "lock deps",
    ]);
    expect(parseNoteConstraints("耗时控制在 10 秒内，锁定依赖版本， 不改接口")).toEqual([
      "耗时控制在 10 秒内",
      "锁定依赖版本",
      "不改接口",
    ]);
    expect(parseNoteConstraints("a, b， c ,, ， ")).toEqual(["a", "b", "c"]);
    expect(parseNoteConstraints("   ")).toEqual([]);
  });
});

describe("formatError.ts mappings and helpers", () => {
  const tEn = i18n.getFixedT("en");
  const tZh = i18n.getFixedT("zh-CN");

  test("strips updated Rust RelayError prefixes and maps common errors", () => {
    expect(
      formatErrorMessage(
        "package manager (uv) error: invalid command preset 'bench' (not declared in relay.toml)",
        tEn,
      ),
    ).toBe('Task preset "bench" is not declared in relay.toml.');
    expect(
      formatErrorMessage(
        "package manager (cargo) error: invalid command preset 'bench' (not declared in relay.toml)",
        tZh,
      ),
    ).toBe("任务预设「bench」未在 relay.toml 中声明。");

    expect(
      formatErrorMessage(
        "internal error: Selected folder is not a Git repository (missing .git). Run `git init` first.",
        tZh,
      ),
    ).toBe(tZh("errors.notGitRepo"));

    expect(formatErrorMessage("io error: Folder does not exist: D:/missing/repo", tEn)).toBe(
      "Folder not found: D:/missing/repo",
    );
    expect(
      formatErrorMessage("internal error: Repository path is not a directory: D:/file.txt", tZh),
    ).toBe("该路径不是有效目录：D:/file.txt");

    expect(
      formatErrorMessage("transport error: Saved address belongs to a different device", tEn),
    ).toBe(tEn("errors.savedAddressDifferentDevice"));
    expect(formatErrorMessage("internal error: Message cannot be empty", tZh)).toBe(
      tZh("errors.emptyMessage"),
    );
    expect(formatErrorMessage("llm client error: expected an HTTP(S) API base URL", tEn)).toBe(
      tEn("errors.invalidLlmBaseUrl"),
    );

    expect(
      formatErrorMessage(
        "toml parse error: Refusing to overwrite .mcp.json: existing file contains invalid JSON (expected value)",
        tEn,
      ),
    ).toBe(
      "Cannot update .mcp.json because it contains invalid JSON. Fix or remove the file first.",
    );
    expect(
      formatErrorMessage(
        'Refusing to overwrite .cursor/mcp.json: "mcpServers" must be a JSON object.',
        tZh,
      ),
    ).toBe("无法更新 .cursor/mcp.json，现有 JSON 结构不兼容，请先修复或删除该文件。");

    expect(formatErrorMessage("storage error: database is locked", tEn)).toBe("database is locked");
  });

  test("detects MCP_PREVIEW_CHANGED and LLM configuration errors accurately", () => {
    expect(
      isMcpPreviewChangedError(
        new Error(
          "The project MCP configuration has changed. Refresh the preview before writing.",
        ),
      ),
    ).toBe(true);
    expect(isMcpPreviewChangedError("MCP_PREVIEW_CHANGED")).toBe(true);
    expect(isMcpPreviewChangedError("Open a local Git repository first.")).toBe(false);

    expect(isLlmConfigurationError("HTTP 401 from /models")).toBe(true);
    expect(isLlmConfigurationError("Connection refused to Ollama endpoint")).toBe(true);
    expect(isLlmConfigurationError("Enter a valid http:// or https:// API base URL.")).toBe(true);
    expect(isLlmConfigurationError("请输入有效的 http:// 或 https:// API 接口地址。")).toBe(true);
    expect(isLlmConfigurationError("Enter a message before sending.")).toBe(false);
    expect(isLlmConfigurationError("发送前请输入消息内容。")).toBe(false);
  });

  test("formats backend task preflight and cancellation summaries cleanly", () => {
    expect(formatTaskSummary("Smoke preflight passed", tEn)).toBe("Quick check passed");
    expect(formatTaskSummary("Smoke preflight passed", tZh)).toBe("快速检查已通过");
    expect(formatTaskSummary("Smoke preflight failed (exit code Some(1))", tEn)).toBe(
      "Quick check failed (exit code 1)",
    );
    expect(formatTaskSummary("Quick check failed (exit code 2)", tZh)).toBe(
      "快速检查失败（退出码 2）",
    );
    expect(formatTaskSummary("Smoke preflight failed (exit code None)", tEn)).toBe(
      "Quick check failed",
    );
    expect(formatTaskSummary("Quick check failed (terminated by signal)", tZh)).toBe(
      "快速检查失败（进程被信号终止）",
    );
    expect(formatTaskSummary("Smoke preflight failed: missing dependency numpy", tEn)).toBe(
      "Quick check failed: missing dependency numpy",
    );
    expect(formatTaskSummary("Cancelled by user", tZh)).toBe("已由用户取消");

    const partial = formatTaskSummary("Cancelled by user (partial log blake3:a1b2c3d4e5f6)", tEn);
    expect(partial).toBe("Cancelled by user (partial log saved)");
    expect(partial).not.toContain("blake3:");
  });

  test("formats LLM probe results and device origins cleanly", () => {
    expect(formatLlmProbeMessage("Connected (14 ms) via /chat/completions", tZh)).toBe(
      "已通过 /chat/completions 连接成功（14 ms）。",
    );
    expect(formatLlmProbeMessage("Connected (9 ms) — model 'qwen3.5:4b' available", tEn)).toBe(
      'Connected (9 ms) — model "qwen3.5:4b" is available.',
    );
    expect(formatLlmProbeMessage("HTTP 401 from /models", tZh)).toBe(
      "接口 /models 返回 HTTP 401。",
    );

    expect(formatDeviceOrigin("relay-copilot", tEn)).toBe("SyncEverything Agent");
    expect(formatDeviceOrigin("relay-copilot", tZh)).toBe("SyncEverything Agent");
    expect(formatDeviceOrigin("SyncEverything Agent", tZh)).toBe("SyncEverything Agent");
    expect(formatDeviceOrigin("Workstation", tZh)).toBe("算力主机");
    expect(formatDeviceOrigin("Laptop", tZh)).toBe("笔记本");
    expect(formatDeviceOrigin("My Custom Rig", tEn)).toBe("My Custom Rig");
  });
});

describe("bilingual locale integrity", () => {
  test("en.json and zh-CN.json have 100% recursive key symmetry and safe distinct copy", () => {
    const enKeys = collectLeafKeysRecursive(en);
    const zhKeys = collectLeafKeysRecursive(zhCN);
    expect(enKeys).toEqual(zhKeys);

    // Safety collision fix: Apply Fix & Run vs Approve & Run must be distinct in ZH
    expect(zhCN.detail.applyAndRun).toBe("应用修复并运行");
    expect(zhCN.detail.approveRun).toBe("确认并运行");
    expect(zhCN.detail.applyAndRun).not.toBe(zhCN.detail.approveRun);

    // Pluralization in EN and ZH
    const tEn = i18n.getFixedT("en");
    const tZh = i18n.getFixedT("zh-CN");
    expect(tEn("canvas.outdatedBanner", { count: 1 })).toBe(
      "1 result is from an older code version and won't be used by AI.",
    );
    expect(tEn("canvas.outdatedBanner", { count: 3 })).toBe(
      "3 results are from older code versions and won't be used by AI.",
    );
    expect(tZh("canvas.outdatedBanner", { count: 3 })).toBe(
      "有 3 项结果基于旧代码，AI 助手已自动忽略，不会误用。",
    );

    // Every MCP HTTP URL error code is present in both locales
    const codes: McpHttpUrlErrorCode[] = [
      "empty",
      "control_chars",
      "whitespace",
      "invalid_scheme",
      "missing_host",
      "embedded_credentials",
      "invalid_url",
      "invalid_host",
      "invalid_port",
    ];
    for (const code of codes) {
      expect(en.modals.workspace.mcpHttpUrlErrors[code].length).toBeGreaterThan(0);
      expect(zhCN.modals.workspace.mcpHttpUrlErrors[code].length).toBeGreaterThan(0);
    }

    // Dead keys removed
    for (const deadKey of [
      "topbar.switchRepo",
      "workspaceSwitcher.activeBadge",
      "modals.task.paramsLabel",
      "modals.pair.devicesTab",
      "modals.pair.nodeIdLabel",
      "modals.pair.pairedAt",
      "modals.workspace.currentRepo",
    ]) {
      expect(enKeys).not.toContain(deadKey);
      expect(zhKeys).not.toContain(deadKey);
    }

    // No raw markdown backticks in plain UI strings
    for (const key of [
      "modals.pair.pastePlaceholder",
      "modals.workspace.mcpHttpUrlHint",
      "modals.workspace.mcpHttpHint",
      "modals.workspace.mcpStdioHint",
    ]) {
      expect(tEn(key as never)).not.toContain("`");
      expect(tZh(key as never)).not.toContain("`");
    }
  });
});
