import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  GitBranch,
  Globe,
  Link2,
  Moon,
  Play,
  SlidersHorizontal,
  Sun,
} from "lucide-react";
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
  onOpenRepoSettings: () => void;
  onOpenLlmSettings: () => void;
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
  onOpenRepoSettings,
  onOpenLlmSettings,
}: TopTelemetryBarProps) {
  const { t, i18n } = useTranslation();
  const currentLang: RelayLocale = i18n.language === "zh-CN" ? "zh-CN" : "en";
  const isConnected = status?.connected ?? false;
  const transport = status?.transport_path ?? "offline";

  return (
    <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-border bg-background px-4">
      <div className="flex min-w-0 items-center gap-2">
        <div className="flex items-center gap-2">
          <div className="flex size-7 shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground shadow-xs">
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
          <span
            lang="en"
            className="relay-text-trim text-sm leading-none font-semibold tracking-tight text-foreground"
          >
            {t("app.name")}
          </span>
        </div>

        <span lang="en" className="relay-text-trim text-xs leading-none text-border select-none">
          /
        </span>

        <Button
          variant="ghost"
          size="default"
          onClick={onOpenRepoSettings}
          title={t("topbar.switchRepo")}
          aria-label={t("topbar.switchRepo")}
          className="min-w-0 gap-2 px-2"
        >
          <GitBranch data-icon="inline-start" className="text-muted-foreground" />
          <span lang="en" className="truncate text-xs leading-snug font-medium text-foreground">
            {repoId}
          </span>
          {headCommitSha && (
            <span
              lang="en"
              className="font-mono-tech relay-text-trim hidden rounded-sm bg-muted px-1.5 py-1 text-[11px] leading-none text-muted-foreground sm:inline-flex sm:items-center"
            >
              {headCommitSha.slice(0, 7)}
            </span>
          )}
        </Button>
      </div>

      <div className="flex items-center gap-2">
        {staleCount > 0 && (
          <Badge variant="destructive" className="gap-1">
            <AlertCircle data-icon="inline-start" />
            <span className="relay-text-trim leading-none">
              {t("topbar.outdatedCount", { count: staleCount })}
            </span>
          </Badge>
        )}

        <Button variant="outline" size="default" onClick={onOpenPairModal}>
          <span
            className={cn(
              "size-2 shrink-0 rounded-full",
              isConnected ? "bg-emerald-500" : "bg-amber-500",
            )}
          />
          <span className="relay-text-trim leading-none">
            {isConnected
              ? `${t(`topbar.transport.${transport}`)}${status?.rtt_ms != null ? ` · ${status.rtt_ms}ms` : ""}`
              : t("topbar.pairDevice")}
          </span>
          <Link2 data-icon="inline-end" className="text-muted-foreground" />
        </Button>

        <Button variant="default" size="default" onClick={onOpenRunTask}>
          <Play data-icon="inline-start" />
          <span className="relay-text-trim leading-none">{t("topbar.runTask")}</span>
        </Button>

        <Separator orientation="vertical" className="my-auto h-4 data-vertical:self-center" />

        <Button
          variant="ghost"
          size="default"
          aria-label={t("topbar.switchLang")}
          onClick={() => setRelayLocale(currentLang === "en" ? "zh-CN" : "en")}
        >
          <Globe data-icon="inline-start" />
          <span
            lang={currentLang === "en" ? "zh-CN" : "en"}
            className="relay-text-trim leading-none"
          >
            {currentLang === "en" ? "中文" : "EN"}
          </span>
        </Button>

        <Button
          variant="ghost"
          size="icon"
          aria-label={t("topbar.openSettings")}
          title={t("topbar.openSettings")}
          onClick={onOpenLlmSettings}
        >
          <SlidersHorizontal />
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
