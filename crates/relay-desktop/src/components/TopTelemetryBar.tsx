import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";
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
    <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-border bg-background px-4">
      <div className="flex min-w-0 items-center gap-2.5">
        <div className="flex items-center gap-2">
          <div className="flex size-7 items-center justify-center rounded-lg bg-primary text-primary-foreground shadow-xs">
            <svg
              viewBox="0 0 24 24"
              className="size-4"
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
          <span className="text-sm font-semibold tracking-tight text-foreground">
            {t("app.name")}
          </span>
        </div>

        <span className="text-border">/</span>

        <div className="flex min-w-0 items-center gap-2">
          <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate text-xs font-medium text-foreground">{repoId}</span>
          {headCommitSha && (
            <span className="font-mono-tech hidden rounded-md bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground sm:inline">
              {headCommitSha.slice(0, 7)}
            </span>
          )}
        </div>
      </div>

      <div className="flex items-center gap-2">
        {staleCount > 0 && (
          <Badge variant="destructive" className="h-6 gap-1 px-2 text-[11px]">
            <AlertCircle data-icon="inline-start" />
            {t("topbar.outdatedCount", { count: staleCount })}
          </Badge>
        )}

        <Button
          variant="outline"
          size="default"
          className="rounded-full px-2.5"
          onClick={onOpenPairModal}
        >
          <span
            className={cn("size-2 rounded-full", isConnected ? "bg-emerald-500" : "bg-amber-500")}
          />
          <span>
            {isConnected
              ? `${t(`topbar.transport.${transport}`)}${status?.rtt_ms != null ? ` · ${status.rtt_ms}ms` : ""}`
              : t("topbar.pairDevice")}
          </span>
          <Link2 data-icon="inline-end" className="text-muted-foreground" />
        </Button>

        <Button variant="default" size="default" className="px-3" onClick={onOpenRunTask}>
          <Play data-icon="inline-start" />
          {t("topbar.runTask")}
        </Button>

        <Separator orientation="vertical" className="h-4" />

        <Button
          variant="ghost"
          size="default"
          aria-label={t("topbar.switchLang")}
          onClick={() => setRelayLocale(currentLang === "en" ? "zh-CN" : "en")}
        >
          <Globe data-icon="inline-start" />
          {currentLang === "en" ? "中文" : "EN"}
        </Button>

        <Button
          variant="ghost"
          size="icon"
          aria-label={t("topbar.toggleTheme")}
          onClick={onToggleTheme}
        >
          {theme === "light" ? <Moon /> : <Sun />}
        </Button>
      </div>
    </header>
  );
}
