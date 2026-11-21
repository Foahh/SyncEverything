import { Button, Chip } from "@heroui/react";
import {
  GitBranch,
  Globe,
  Moon,
  Radio,
  ShieldAlert,
  ShieldCheck,
  Sun,
  Wifi,
  WifiOff,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PeerStatus_Serialize, RuntimeMode, TransportPath } from "../bindings";
import { setRelayLocale, type RelayLocale } from "../i18n";
import { isTauriRuntime } from "../lib/relayApi";

interface TopTelemetryBarProps {
  status: PeerStatus_Serialize | undefined;
  repoId: string;
  headCommitSha: string | null;
  staleCount: number;
  theme: "light" | "dark";
  onToggleTheme: () => void;
  onSwitchMode: (mode: RuntimeMode) => void;
}

const RUNTIME_MODES: RuntimeMode[] = ["client", "host", "local_dual_sandbox"];

function transportStyle(path: TransportPath): {
  color: "success" | "accent" | "warning" | "default";
  Icon: typeof Wifi;
} {
  switch (path) {
    case "direct_lan":
      return { color: "success", Icon: Wifi };
    case "relay":
      return { color: "accent", Icon: Radio };
    case "loopback":
      return { color: "success", Icon: Radio };
    case "offline":
      return { color: "warning", Icon: WifiOff };
  }
}

export function TopTelemetryBar({
  status,
  repoId,
  headCommitSha,
  staleCount,
  theme,
  onToggleTheme,
  onSwitchMode,
}: TopTelemetryBarProps) {
  const { t, i18n } = useTranslation();
  const currentLang: RelayLocale = i18n.language === "zh-CN" ? "zh-CN" : "en";
  const activeMode: RuntimeMode = status?.mode ?? "local_dual_sandbox";
  const activeTransport: TransportPath = status?.transport_path ?? "loopback";
  const { color: transportColor, Icon: TransportIcon } = transportStyle(activeTransport);

  return (
    <header className="flex h-[54px] shrink-0 items-center justify-between gap-3 border-b border-[var(--relay-border)] bg-[var(--relay-surface)] px-4">
      {/* Left: Brand mark + Repo / HEAD telemetry */}
      <div className="flex items-center gap-3 min-w-0">
        <div className="flex items-center gap-2.5">
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-slate-900 text-teal-400 shadow-xs dark:bg-teal-500/15 dark:text-teal-300 dark:ring-1 dark:ring-teal-500/30">
            <svg
              viewBox="0 0 24 24"
              className="h-4 w-4"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
            >
              <circle cx="6" cy="12" r="2.5" />
              <circle cx="18" cy="6" r="2.5" />
              <circle cx="18" cy="18" r="2.5" />
              <path d="M8.5 11.2L15.6 7.2M8.5 12.8L15.6 16.8" strokeLinecap="round" />
            </svg>
          </div>
          <div className="leading-tight">
            <div className="flex items-center gap-1.5">
              <span className="text-[14px] font-bold tracking-tight text-[var(--relay-text)]">
                {t("app.name")}
              </span>
              <span className="font-mono-tech rounded border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] px-1.5 py-0.5 text-[9.5px] font-medium text-[var(--relay-text-faint)]">
                {isTauriRuntime() ? t("app.tauriBadge") : t("app.previewBadge")}
              </span>
            </div>
            <p className="text-[10.5px] text-[var(--relay-text-faint)]">{t("app.subtitle")}</p>
          </div>
        </div>

        <div className="hidden h-5 w-px bg-[var(--relay-border)] md:block" />

        <div className="hidden items-center gap-2 rounded-lg border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] px-2.5 py-1 text-[11.5px] md:flex">
          <GitBranch className="h-3.5 w-3.5 text-teal-600 dark:text-teal-400" />
          <span className="font-semibold text-[var(--relay-text)]">{repoId}</span>
          {headCommitSha && (
            <span className="font-mono-tech rounded bg-[var(--relay-surface)] px-1.5 py-0.5 text-[10.5px] text-[var(--relay-text-muted)] border border-[var(--relay-border)]">
              {t("topbar.headLabel")} @{headCommitSha.slice(0, 7)}
            </span>
          )}
        </div>
      </div>

      {/* Center: Runtime Mode Segmented Switcher */}
      <div className="hidden items-center rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] p-0.5 lg:flex">
        {RUNTIME_MODES.map((mode) => {
          const isActive = mode === activeMode;
          return (
            <button
              key={mode}
              type="button"
              onClick={() => onSwitchMode(mode)}
              className={[
                "rounded-[9px] px-2.5 py-1 text-[11.5px] font-medium transition-all cursor-pointer",
                isActive
                  ? "bg-[var(--relay-surface)] text-[var(--relay-text)] shadow-xs ring-1 ring-[var(--relay-border)]"
                  : "text-[var(--relay-text-muted)] hover:text-[var(--relay-text)]",
              ].join(" ")}
            >
              {t(`topbar.mode.${mode}`)}
            </button>
          );
        })}
      </div>

      {/* Right: Transport Link + Freshness Pill + Language + Theme */}
      <div className="flex items-center gap-2">
        <Chip
          size="sm"
          variant="soft"
          color={transportColor}
          className="font-mono-tech h-7 px-2.5 text-[11px] font-medium"
        >
          <TransportIcon className="mr-1 h-3.5 w-3.5" />
          <span>{t(`topbar.transport.${activeTransport}`)}</span>
          {status?.rtt_ms != null && <span className="ml-1 opacity-75">{status.rtt_ms}ms</span>}
        </Chip>

        <Chip
          size="sm"
          variant={staleCount > 0 ? "primary" : "soft"}
          color={staleCount > 0 ? "danger" : "success"}
          className="h-7 px-2.5 text-[11px] font-semibold"
        >
          {staleCount > 0 ? (
            <>
              <ShieldAlert className="mr-1 h-3.5 w-3.5" />
              {t("topbar.stalePill", { count: staleCount })}
            </>
          ) : (
            <>
              <ShieldCheck className="mr-1 h-3.5 w-3.5" />
              {t("topbar.allFresh")}
            </>
          )}
        </Chip>

        <div className="h-5 w-px bg-[var(--relay-border)]" />

        <Button
          size="sm"
          variant="ghost"
          aria-label={t("topbar.switchLang")}
          onPress={() => setRelayLocale(currentLang === "en" ? "zh-CN" : "en")}
          className="font-mono-tech h-7 min-w-0 px-2 text-[11px]"
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
