import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  Check,
  CheckCircle2,
  Copy,
  Cpu,
  FolderGit2,
  Info,
  Laptop,
  Link2,
  MonitorSmartphone,
  Pencil,
  RefreshCw,
  ShieldBan,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  PeerRole,
  PeerStatus_Serialize,
  RuntimeMode,
  TrustedPeerRecord_Serialize,
  WorkspaceInfo_Serialize,
} from "../bindings";
import { formatErrorMessage } from "../lib/formatError";
import {
  useConnectPeerTicket,
  useGetPairingTicket,
  useReconnectTrustedPeer,
  useRemoveTrustedPeer,
  useRenameTrustedPeer,
  useRevokeTrustedPeer,
  useSwitchRuntimeMode,
  useTrustedPeers,
} from "../lib/queries";

interface DevicesViewProps {
  workspace: WorkspaceInfo_Serialize | undefined;
  status: PeerStatus_Serialize | undefined;
  onGoToWorkspace: () => void;
}

const RUNTIME_ROLE_CARDS: ReadonlyArray<{
  mode: RuntimeMode;
  icon: typeof Cpu;
  descKey:
    | "devices.roleDesc.client"
    | "devices.roleDesc.host"
    | "devices.roleDesc.local_dual_sandbox";
}> = [
  { mode: "client", icon: Laptop, descKey: "devices.roleDesc.client" },
  { mode: "host", icon: Cpu, descKey: "devices.roleDesc.host" },
  {
    mode: "local_dual_sandbox",
    icon: MonitorSmartphone,
    descKey: "devices.roleDesc.local_dual_sandbox",
  },
];

function abbreviateNodeId(nodeId: string): string {
  if (nodeId.length <= 16) return nodeId;
  return `${nodeId.slice(0, 8)}…${nodeId.slice(-6)}`;
}

function formatPeerTimestamp(iso: string, locale: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  try {
    return new Intl.DateTimeFormat(locale, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(date);
  } catch {
    return date.toLocaleString();
  }
}

function RoleIcon({ role }: { role: PeerRole }) {
  if (role === "host") return <Cpu className="size-3.5" aria-hidden="true" />;
  if (role === "client") return <Laptop className="size-3.5" aria-hidden="true" />;
  return <MonitorSmartphone className="size-3.5" aria-hidden="true" />;
}

interface TrustedDeviceRowProps {
  peer: TrustedPeerRecord_Serialize;
  mode: RuntimeMode;
  isConnected: boolean;
  copiedNodeId: string | null;
  isBusy: boolean;
  isReconnecting: boolean;
  onCopyNodeId: (nodeId: string) => void;
  onRename: (nodeId: string, nextLabel: string) => Promise<boolean>;
  onReconnect: (peer: TrustedPeerRecord_Serialize) => void;
  onRevoke: (peer: TrustedPeerRecord_Serialize) => void;
  onRemove: (peer: TrustedPeerRecord_Serialize) => void;
}

function TrustedDeviceRow({
  peer,
  mode,
  isConnected,
  copiedNodeId,
  isBusy,
  isReconnecting,
  onCopyNodeId,
  onRename,
  onReconnect,
  onRevoke,
  onRemove,
}: TrustedDeviceRowProps) {
  const { t, i18n } = useTranslation();
  const [isEditing, setIsEditing] = useState(false);
  const [draftLabel, setDraftLabel] = useState(peer.label);
  const [confirmingRevoke, setConfirmingRevoke] = useState(false);

  const isRevoked = peer.revoked_at !== null;
  const isCopied = copiedNodeId === peer.node_id;
  const renameInputId = `rename-peer-${peer.node_id.slice(0, 12)}`;

  const startEditing = () => {
    setDraftLabel(peer.label);
    setIsEditing(true);
  };

  const submitRename = async () => {
    const trimmed = draftLabel.trim();
    if (!trimmed || isBusy) return;
    if (trimmed === peer.label || (await onRename(peer.node_id, trimmed))) {
      setIsEditing(false);
    }
  };

  return (
    <li className="flex flex-col gap-2.5 px-4 py-3">
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 flex-1 items-center gap-2.5">
          <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-muted text-foreground">
            <RoleIcon role={peer.role} />
          </span>

          {isEditing ? (
            <form
              className="flex min-w-0 flex-1 items-center gap-1.5"
              onSubmit={(event) => {
                event.preventDefault();
                void submitRename();
              }}
            >
              <label htmlFor={renameInputId} className="sr-only">
                {t("modals.pair.actions.rename")} {peer.label}
              </label>
              <Input
                id={renameInputId}
                value={draftLabel}
                onChange={(event) => setDraftLabel(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    setIsEditing(false);
                    setDraftLabel(peer.label);
                  }
                }}
                placeholder={t("modals.pair.renamePlaceholder")}
                className="h-7 flex-1 text-xs"
                autoFocus
                required
              />
              <Button
                type="submit"
                size="icon-sm"
                variant="default"
                disabled={!draftLabel.trim() || isBusy}
                aria-label={`${t("modals.pair.actions.saveRename")} ${peer.label}`}
              >
                <Check aria-hidden="true" />
              </Button>
              <Button
                type="button"
                size="icon-sm"
                variant="ghost"
                onClick={() => {
                  setIsEditing(false);
                  setDraftLabel(peer.label);
                }}
                aria-label={`${t("modals.pair.actions.cancelRename")} ${peer.label}`}
              >
                <X aria-hidden="true" />
              </Button>
            </form>
          ) : (
            <div className="flex min-w-0 items-center gap-1.5">
              <span className="truncate text-xs font-semibold text-foreground">{peer.label}</span>
              <Button
                type="button"
                size="icon-xs"
                variant="ghost"
                disabled={isBusy}
                onClick={startEditing}
                aria-label={`${t("modals.pair.actions.rename")} ${peer.label}`}
                title={t("modals.pair.actions.rename")}
                className="text-muted-foreground hover:text-foreground"
              >
                <Pencil aria-hidden="true" />
              </Button>
            </div>
          )}
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          <Badge variant="outline">
            <span className="relay-text-trim leading-none">
              {t(`modals.pair.role.${peer.role}`)}
            </span>
          </Badge>
          {isRevoked ? (
            <Badge variant="destructive">
              <span className="relay-text-trim leading-none">
                {t("modals.pair.status.revoked")}
              </span>
            </Badge>
          ) : isConnected ? (
            <Badge
              variant="secondary"
              className="bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300"
            >
              <span className="size-1.5 rounded-full bg-emerald-500" aria-hidden="true" />
              <span className="relay-text-trim leading-none">
                {t("modals.pair.status.connected")}
              </span>
            </Badge>
          ) : (
            <Badge variant="secondary" className="text-muted-foreground">
              <span className="relay-text-trim leading-none">
                {t("modals.pair.status.offline")}
              </span>
            </Badge>
          )}
        </div>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground">
        <div className="flex min-w-0 items-center gap-1.5">
          <code
            lang="en"
            title={peer.node_id}
            className="font-mono-tech rounded bg-muted/70 px-1.5 py-0.5 text-[10.5px] text-foreground/85"
          >
            {abbreviateNodeId(peer.node_id)}
          </code>
          <Button
            type="button"
            size="icon-xs"
            variant="ghost"
            onClick={() => onCopyNodeId(peer.node_id)}
            aria-label={`${isCopied ? t("modals.pair.copiedNodeId") : t("modals.pair.copyNodeId")} ${peer.label}`}
            title={isCopied ? t("modals.pair.copiedNodeId") : t("modals.pair.copyNodeId")}
            className="text-muted-foreground hover:text-foreground"
          >
            {isCopied ? (
              <Check className="text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
            ) : (
              <Copy aria-hidden="true" />
            )}
          </Button>
        </div>

        <time dateTime={peer.last_seen_at} title={peer.last_seen_at} className="font-mono-tech">
          {t("modals.pair.lastSeen", {
            time: formatPeerTimestamp(peer.last_seen_at, i18n.language),
          })}
        </time>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 pt-0.5">
        {isRevoked ? (
          <p className="text-[11px] leading-snug text-destructive/90">
            {t("modals.pair.revokedHint")}
          </p>
        ) : confirmingRevoke ? (
          <div className="flex w-full flex-col gap-2">
            <p className="text-[11px] leading-snug text-destructive/90">
              {t("modals.pair.revokeConfirmHint")}
            </p>
            <div className="flex items-center justify-end gap-1.5">
              <Button
                type="button"
                size="xs"
                variant="outline"
                disabled={isBusy}
                onClick={() => setConfirmingRevoke(false)}
              >
                <span className="relay-text-trim leading-none">
                  {t("modals.pair.actions.cancelRename")}
                </span>
              </Button>
              <Button
                type="button"
                size="xs"
                variant="destructive"
                disabled={isBusy}
                onClick={() => {
                  setConfirmingRevoke(false);
                  onRevoke(peer);
                }}
              >
                <ShieldBan data-icon="inline-start" aria-hidden="true" />
                <span className="relay-text-trim leading-none">
                  {t("modals.pair.actions.confirmRevoke")}
                </span>
              </Button>
            </div>
          </div>
        ) : (
          <div className="flex items-center gap-1.5">
            <Button
              type="button"
              size="xs"
              variant="outline"
              disabled={isConnected || isBusy || mode === "local_dual_sandbox"}
              onClick={() => onReconnect(peer)}
              aria-label={`${t("modals.pair.actions.reconnect")} ${peer.label}`}
            >
              <RefreshCw
                data-icon="inline-start"
                className={isReconnecting ? "animate-spin" : undefined}
                aria-hidden="true"
              />
              <span className="relay-text-trim leading-none">
                {isReconnecting
                  ? t("modals.pair.actions.reconnecting")
                  : t("modals.pair.actions.reconnect")}
              </span>
            </Button>
            <Button
              type="button"
              size="xs"
              variant="destructive"
              disabled={isBusy}
              onClick={() => setConfirmingRevoke(true)}
              aria-label={`${t("modals.pair.actions.revoke")} ${peer.label}`}
            >
              <ShieldBan data-icon="inline-start" aria-hidden="true" />
              <span className="relay-text-trim leading-none">
                {t("modals.pair.actions.revoke")}
              </span>
            </Button>
          </div>
        )}

        {!confirmingRevoke && (
          <Button
            type="button"
            size="xs"
            variant="ghost"
            disabled={isBusy}
            onClick={() => onRemove(peer)}
            aria-label={`${t("modals.pair.actions.remove")} ${peer.label}`}
            className="ml-auto text-muted-foreground hover:text-destructive"
          >
            <Trash2 data-icon="inline-start" aria-hidden="true" />
            <span className="relay-text-trim leading-none">{t("modals.pair.actions.remove")}</span>
          </Button>
        )}
      </div>
    </li>
  );
}

type PeerActionKind = "rename" | "reconnect" | "revoke" | "remove";

export function DevicesView({ workspace, status, onGoToWorkspace }: DevicesViewProps) {
  const { t } = useTranslation();
  const mode: RuntimeMode = status?.mode ?? "local_dual_sandbox";
  const hasRepo = Boolean(workspace?.repo_root);
  const [tab, setTab] = useState<"share" | "connect">(mode === "client" ? "connect" : "share");
  const [ticket, setTicket] = useState("");
  const [copied, setCopied] = useState(false);
  const [copiedNodeId, setCopiedNodeId] = useState<string | null>(null);
  const [remoteInput, setRemoteInput] = useState("");
  const [busyByNode, setBusyByNode] = useState<Record<string, PeerActionKind>>({});
  const busyNodesRef = useRef(new Set<string>());
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const { data: trustedPeers = [] } = useTrustedPeers();
  const switchMode = useSwitchRuntimeMode();
  const getTicket = useGetPairingTicket();
  const connectPeer = useConnectPeerTicket();
  const renamePeer = useRenameTrustedPeer();
  const reconnectPeer = useReconnectTrustedPeer();
  const revokePeer = useRevokeTrustedPeer();
  const removePeer = useRemoveTrustedPeer();

  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 1800);
    return () => window.clearTimeout(id);
  }, [copied]);

  useEffect(() => {
    if (!copiedNodeId) return;
    const id = window.setTimeout(() => setCopiedNodeId(null), 1800);
    return () => window.clearTimeout(id);
  }, [copiedNodeId]);

  const handleGenerate = () => {
    setCopied(false);
    getTicket.mutate(undefined, {
      onSuccess: (generated) => {
        setTicket(generated);
        setFeedback(null);
      },
      onError: (error) => setFeedback({ ok: false, text: formatErrorMessage(error, t) }),
    });
  };

  const handleCopy = async () => {
    if (!ticket) return;
    try {
      await navigator.clipboard.writeText(ticket);
      setCopied(true);
      setFeedback(null);
    } catch {
      setFeedback({ ok: false, text: t("modals.pair.copyFailed") });
    }
  };

  const handleCopyNodeId = async (nodeId: string) => {
    try {
      await navigator.clipboard.writeText(nodeId);
      setCopiedNodeId(nodeId);
    } catch {
      setFeedback({ ok: false, text: t("modals.pair.copyFailed") });
    }
  };

  const handleConnect = () => {
    const trimmed = remoteInput.trim();
    if (!trimmed) return;
    setFeedback(null);
    connectPeer.mutate(trimmed, {
      onSuccess: () => {
        setFeedback({ ok: true, text: t("modals.pair.connectedSuccess") });
        setRemoteInput("");
      },
      onError: (err) => {
        setFeedback({
          ok: false,
          text: formatErrorMessage(err, t, t("modals.pair.invalidCode")),
        });
      },
    });
  };

  const runPeerAction = async (
    action: { nodeId: string; kind: PeerActionKind },
    mutate: () => Promise<string>,
  ) => {
    if (busyNodesRef.current.has(action.nodeId)) return false;
    busyNodesRef.current.add(action.nodeId);
    setBusyByNode((prev) => ({ ...prev, [action.nodeId]: action.kind }));
    setFeedback(null);
    try {
      setFeedback({ ok: true, text: await mutate() });
      return true;
    } catch (error) {
      setFeedback({ ok: false, text: formatErrorMessage(error, t) });
      return false;
    } finally {
      busyNodesRef.current.delete(action.nodeId);
      setBusyByNode((prev) => {
        const next = { ...prev };
        delete next[action.nodeId];
        return next;
      });
    }
  };

  const transport = status?.transport_path ?? "offline";
  const isConnected = status?.connected ?? false;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto overscroll-contain bg-background [scrollbar-gutter:stable]">
      <div className="flex w-full max-w-3xl flex-col gap-8 px-8 py-6">
        <section className="flex flex-col gap-3">
          <div className="flex items-center justify-between gap-3">
            <div>
              <h2 className="text-sm font-semibold text-foreground">
                {t("devices.thisDeviceTitle")}
              </h2>
              <p className="text-xs text-muted-foreground">{t("devices.thisDeviceDesc")}</p>
            </div>
            <Badge
              variant={isConnected ? "secondary" : "outline"}
              className={
                isConnected
                  ? "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300"
                  : undefined
              }
            >
              <span
                aria-hidden="true"
                className={cn(
                  "size-1.5 rounded-full",
                  isConnected
                    ? "bg-emerald-500"
                    : mode === "local_dual_sandbox"
                      ? "bg-sky-500"
                      : "bg-amber-500",
                )}
              />
              <span className="relay-text-trim leading-none">
                {t(`topbar.transport.${transport}`)}
                {status?.rtt_ms != null ? ` · ${status.rtt_ms}ms` : ""}
              </span>
            </Badge>
          </div>

          <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
            {RUNTIME_ROLE_CARDS.map(({ mode: itemMode, icon: Icon, descKey }) => {
              const active = itemMode === mode;
              return (
                <button
                  key={itemMode}
                  type="button"
                  disabled={switchMode.isPending}
                  aria-pressed={active}
                  onClick={() => {
                    switchMode.mutate(itemMode);
                    setTicket("");
                    setFeedback(null);
                    if (itemMode === "client") setTab("connect");
                    if (itemMode === "host") setTab("share");
                  }}
                  className={cn(
                    "flex cursor-pointer flex-col gap-1.5 rounded-lg border p-3.5 text-left transition-all disabled:pointer-events-none disabled:opacity-50",
                    active
                      ? "border-primary bg-primary/6 ring-1 ring-primary/20"
                      : "border-border bg-card hover:border-foreground/20",
                  )}
                >
                  <div className="flex items-center gap-2 text-xs font-semibold text-foreground">
                    <Icon
                      className={cn(
                        "size-4 shrink-0",
                        active ? "text-primary" : "text-muted-foreground",
                      )}
                      aria-hidden="true"
                    />
                    <span className="relay-text-trim leading-none">
                      {t(`sidebar.mode.${itemMode}`)}
                    </span>
                  </div>
                  <p className="text-[11.5px] leading-relaxed text-muted-foreground">
                    {t(descKey)}
                  </p>
                </button>
              );
            })}
          </div>

          {switchMode.error && (
            <Alert
              role="alert"
              variant="destructive"
              className="border-destructive/30 bg-[var(--relay-stale-soft)]"
            >
              <AlertCircle aria-hidden="true" />
              <AlertDescription>{formatErrorMessage(switchMode.error, t)}</AlertDescription>
            </Alert>
          )}
        </section>

        <Separator />

        <section className="flex flex-col gap-3">
          <div>
            <h2 className="text-sm font-semibold text-foreground">{t("modals.pair.title")}</h2>
            <p className="text-xs text-muted-foreground">{t("devices.pairDesc")}</p>
          </div>

          {!hasRepo ? (
            <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-dashed border-border bg-muted/25 px-4 py-4">
              <div className="flex min-w-0 items-center gap-2.5 text-xs text-muted-foreground">
                <Info className="size-4 shrink-0 text-primary" aria-hidden="true" />
                <span>{t("modals.workspace.pairRequiresRepoHint")}</span>
              </div>
              <Button type="button" size="sm" variant="outline" onClick={onGoToWorkspace}>
                <FolderGit2 data-icon="inline-start" aria-hidden="true" />
                <span className="relay-text-trim leading-none">
                  {t("modals.workspace.openRepoBtn")}
                </span>
              </Button>
            </div>
          ) : mode === "local_dual_sandbox" ? (
            <div className="rounded-lg border border-dashed border-border bg-muted/25 px-4 py-4 text-xs text-muted-foreground">
              {t("modals.pair.chooseRole")}
            </div>
          ) : (
            <div className="rounded-lg border border-border bg-card p-4">
              <Tabs
                value={tab}
                onValueChange={(value) => setTab(value === "connect" ? "connect" : "share")}
                className="gap-4"
              >
                <TabsList className="grid w-full max-w-xs grid-cols-2">
                  <TabsTrigger value="share">
                    <span className="relay-text-trim leading-none">
                      {t("modals.pair.shareTab")}
                    </span>
                  </TabsTrigger>
                  <TabsTrigger value="connect">
                    <span className="relay-text-trim leading-none">
                      {t("modals.pair.connectTab")}
                    </span>
                  </TabsTrigger>
                </TabsList>

                <TabsContent value="share" className="flex flex-col gap-3">
                  <p className="text-xs leading-relaxed text-muted-foreground">
                    {t("modals.pair.shareDesc")}
                  </p>

                  {ticket ? (
                    <div className="flex flex-col gap-2.5">
                      <pre className="font-mono-tech max-h-36 overflow-auto rounded-md border border-border bg-muted/50 p-3 text-[11px] leading-relaxed text-foreground">
                        {ticket}
                      </pre>
                      <div className="flex items-center gap-2">
                        <Button size="default" onClick={() => void handleCopy()}>
                          {copied ? (
                            <Check data-icon="inline-start" aria-hidden="true" />
                          ) : (
                            <Copy data-icon="inline-start" aria-hidden="true" />
                          )}
                          <span className="relay-text-trim leading-none">
                            {copied ? t("modals.pair.copiedBtn") : t("modals.pair.copyBtn")}
                          </span>
                        </Button>
                        <Button
                          size="default"
                          variant="outline"
                          disabled={getTicket.isPending}
                          onClick={handleGenerate}
                        >
                          <span className="relay-text-trim leading-none">
                            {t("modals.pair.generateBtn")}
                          </span>
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <div>
                      <Button
                        size="default"
                        disabled={getTicket.isPending}
                        onClick={handleGenerate}
                      >
                        <Sparkles data-icon="inline-start" aria-hidden="true" />
                        <span className="relay-text-trim leading-none">
                          {t("modals.pair.generateBtn")}
                        </span>
                      </Button>
                    </div>
                  )}
                </TabsContent>

                <TabsContent value="connect" className="flex flex-col gap-3">
                  <FieldGroup>
                    <Field>
                      <FieldLabel htmlFor="pair-code-input">
                        {t("modals.pair.pasteLabel")}
                      </FieldLabel>
                      <FieldDescription>{t("devices.connectHint")}</FieldDescription>
                      <Textarea
                        id="pair-code-input"
                        rows={3}
                        value={remoteInput}
                        onChange={(e) => setRemoteInput(e.target.value)}
                        placeholder={t("modals.pair.pastePlaceholder")}
                        autoCorrect="off"
                        autoCapitalize="none"
                        spellCheck={false}
                        className="font-mono-tech text-[11.5px]"
                      />
                    </Field>
                  </FieldGroup>

                  <div>
                    <Button
                      size="default"
                      disabled={!remoteInput.trim() || connectPeer.isPending}
                      onClick={handleConnect}
                    >
                      <Link2 data-icon="inline-start" aria-hidden="true" />
                      <span className="relay-text-trim leading-none">
                        {t("modals.pair.connectBtn")}
                      </span>
                    </Button>
                  </div>
                </TabsContent>
              </Tabs>
            </div>
          )}
        </section>

        {feedback && (
          <Alert
            variant={feedback.ok ? "default" : "destructive"}
            role={feedback.ok ? "status" : "alert"}
            aria-live={feedback.ok ? "polite" : "assertive"}
            className={
              feedback.ok
                ? "border-emerald-500/25 bg-[var(--relay-fresh-soft)] text-emerald-800 dark:text-emerald-200"
                : "border-destructive/30 bg-[var(--relay-stale-soft)]"
            }
          >
            {feedback.ok ? (
              <CheckCircle2 className="text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
            ) : (
              <AlertCircle aria-hidden="true" />
            )}
            <AlertDescription
              className={feedback.ok ? "text-emerald-800 dark:text-emerald-200" : undefined}
            >
              {feedback.text}
            </AlertDescription>
          </Alert>
        )}

        <Separator />

        <section className="flex flex-col gap-3">
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-semibold text-foreground">
              {t("modals.pair.trustedTitle")}
            </h2>
            {trustedPeers.length > 0 && (
              <Badge variant="secondary" className="font-mono-tech">
                {trustedPeers.length}
              </Badge>
            )}
          </div>

          {trustedPeers.length === 0 ? (
            <div className="rounded-lg border border-dashed border-border bg-muted/20 px-4 py-6 text-xs text-muted-foreground">
              {t("modals.pair.trustedEmpty")}
            </div>
          ) : (
            <ul
              role="list"
              className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-card"
            >
              {trustedPeers.map((peer) => {
                const peerConnected =
                  peer.revoked_at === null &&
                  Boolean(status?.connected && status.remote_node_id === peer.node_id);
                return (
                  <TrustedDeviceRow
                    key={peer.node_id}
                    peer={peer}
                    mode={mode}
                    isConnected={peerConnected}
                    copiedNodeId={copiedNodeId}
                    isBusy={Boolean(busyByNode[peer.node_id])}
                    isReconnecting={busyByNode[peer.node_id] === "reconnect"}
                    onCopyNodeId={(nodeId) => void handleCopyNodeId(nodeId)}
                    onRename={(nodeId, label) =>
                      runPeerAction({ nodeId, kind: "rename" }, async () => {
                        const updated = await renamePeer.mutateAsync({ nodeId, label });
                        return t("modals.pair.renamedSuccess", { label: updated.label });
                      })
                    }
                    onReconnect={({ node_id: nodeId, label }) =>
                      void runPeerAction({ nodeId, kind: "reconnect" }, async () => {
                        await reconnectPeer.mutateAsync(nodeId);
                        return t("modals.pair.reconnectedSuccess", { label });
                      })
                    }
                    onRevoke={({ node_id: nodeId, label }) =>
                      void runPeerAction({ nodeId, kind: "revoke" }, async () => {
                        await revokePeer.mutateAsync(nodeId);
                        return t("modals.pair.revokedSuccess", { label });
                      })
                    }
                    onRemove={({ node_id: nodeId, label }) =>
                      void runPeerAction({ nodeId, kind: "remove" }, async () => {
                        await removePeer.mutateAsync(nodeId);
                        return t("modals.pair.removedSuccess", { label });
                      })
                    }
                  />
                );
              })}
            </ul>
          )}
        </section>
      </div>
    </div>
  );
}
