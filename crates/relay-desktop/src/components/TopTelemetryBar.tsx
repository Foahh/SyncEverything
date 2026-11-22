import { Button, Chip } from "@heroui/react";
import { AlertCircle, GitBranch, Globe, Link2, Moon, Play, Sun } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PeerStatus_Serialize } from "../bindings";
import { setRelayLocale, type RelayLocale } from "../i18n";

interface TopTelemetryBarProps {
  status: PeerStatus_Serialize | undefined;
  repoId: string;
  headCommitSha: string | null;
  staleCount: number;
  theme: "light" | "dark";
  onToggleTheme: () => void;
  onOpenPairModal: () => void;
  onOpenRunTask: () => void;
}

export function TopTelemetryBar({
  status,
  repoId,
  headCommitSha,
  staleCount,
  theme,
  onToggleTheme,
  onOpenPairModal,
  onOpenRunTask,
}: TopTelemetryBarProps) {
  const { t, i18n } = useTranslation();
  const currentLang: RelayLocale = i18n.language === "zh-CN" ? "zh-CN" : "en";
  const isConnected = status?.connected ?? false;
  const transport = status?.transport_path ?? "offline";

  return (
    <header className="flex h-[52px] shrink-0 items-center justify-between gap-3 border-b border-[var(--relay-border)] bg-[var(--relay-surface)] px-4">
      {/* Left: Clean Brand + Current Project */}
      <div className="flex items-center gap-3 min-w-0">
        <div className="flex items-center gap-2">
          <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-teal-600 text-white shadow-xs">
            <svg
              viewBox="0 0 24 24"
              className="h-4 w-4"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.3"
            >
              <circle cx="6" cy="12" r="2.5" />
              <circle cx="18" cy="6" r="2.5" />
              <circle cx="18" cy="18" r="2.5" />
              <path d="M8.5 11.2L15.6 7.2M8.5 12.8L15.6 16.8" strokeLinecap="round" />
            </svg>
          </div>
          <span className="text-[14.5px] font-bold tracking-tight text-[var(--relay-text)]">
            {t("app.name")}
          </span>
        </div>

        <span className="text-[var(--relay-border-strong)]">/</span>

        <div className="flex items-center gap-2 min-w-0">
          <GitBranch className="h-3.5 w-3.5 shrink-0 text-[var(--relay-text-faint)]" />
          <span className="truncate text-[13px] font-medium text-[var(--relay-text)]">
            {repoId}
          </span>
          {headCommitSha && (
            <span className="font-mono-tech hidden rounded-md bg-[var(--relay-surface-subtle)] px-1.5 py-0.5 text-[11px] text-[var(--relay-text-faint)] sm:inline">
              {headCommitSha.slice(0, 7)}
            </span>
          )}
        </div>
      </div>

      {/* Right: Connection Pill + Outdated Indicator (only when needed) + Primary CTA + Utilities */}
      <div className="flex items-center gap-2">
        {staleCount > 0 && (
          <Chip
            size="sm"
            variant="soft"
            color="danger"
            className="h-7 px-2.5 text-[11.5px] font-medium"
          >
            <AlertCircle className="mr-1 h-3.5 w-3.5" />
            {t("topbar.outdatedCount", { count: staleCount })}
          </Chip>
        )}

        <button
          type="button"
          onClick={onOpenPairModal}
          className="inline-flex h-7 items-center gap-1.5 rounded-full border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] px-2.5 text-[11.5px] font-medium text-[var(--relay-text)] transition-colors hover:border-[var(--relay-border-strong)] cursor-pointer"
        >
          <span
            className={["h-2 w-2 rounded-full", isConnected ? "bg-teal-500" : "bg-amber-500"].join(
              " ",
            )}
          />
          <span>
            {isConnected
              ? `${t(`topbar.transport.${transport}`)}${status?.rtt_ms != null ? ` · ${status.rtt_ms}ms` : ""}`
              : t("topbar.pairDevice")}
          </span>
          <Link2 className="h-3 w-3 text-[var(--relay-text-faint)]" />
        </button>

        <Button
          size="sm"
          variant="primary"
          className="h-7 px-3 text-[12px] font-medium"
          onPress={onOpenRunTask}
        >
          <Play className="h-3 w-3" />
          {t("topbar.runTask")}
        </Button>

        <div className="h-4 w-px bg-[var(--relay-border)]" />

        <Button
          size="sm"
          variant="ghost"
          aria-label={t("topbar.switchLang")}
          onPress={() => setRelayLocale(currentLang === "en" ? "zh-CN" : "en")}
          className="h-7 min-w-0 px-2 text-[11.5px]"
        >
          <Globe className="h-3.5 w-3.5" />
          {currentLang === "en" ? "中文" : "EN"}
        </Button>

        <Button
          size="sm"
          variant="ghost"
          isIconOnly
          aria-label={t("topbar.toggleTheme")}
          onPress={onToggleTheme}
          className="h-7 w-7 min-w-7"
        >
          {theme === "light" ? <Moon className="h-3.5 w-3.5" /> : <Sun className="h-3.5 w-3.5" />}
        </Button>
      </div>
    </header>
  );
}
