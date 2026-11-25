import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import {
  AlertCircle,
  Check,
  CheckCircle2,
  Copy,
  Cpu,
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
} from "../../bindings";
import { formatErrorMessage } from "../../lib/formatError";
import {
  useConnectPeerTicket,
  useGetPairingTicket,
  useReconnectTrustedPeer,
  useRemoveTrustedPeer,
  useRenameTrustedPeer,
  useRevokeTrustedPeer,
  useSwitchRuntimeMode,
  useTrustedPeers,
} from "../../lib/queries";

interface PairDeviceModalProps {
  mode: RuntimeMode;
  status: PeerStatus_Serialize | undefined;
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
}

const ROLE_CARDS: ReadonlyArray<{
  role: "host" | "client";
  icon: typeof Cpu;
  descKey: "modals.pair.roleHostDesc" | "modals.pair.roleClientDesc";
}> = [
  { role: "host", icon: Cpu, descKey: "modals.pair.roleHostDesc" },
  { role: "client", icon: Laptop, descKey: "modals.pair.roleClientDesc" },
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

function PairingRolePicker() {
  const { t } = useTranslation();
  const switchMode = useSwitchRuntimeMode();
  return (
    <div className="flex flex-col gap-3">
      <p className="text-xs leading-relaxed text-muted-foreground">{t("modals.pair.chooseRole")}</p>
      <div className="grid grid-cols-2 gap-2.5">
        {ROLE_CARDS.map(({ role, icon: Icon, descKey }) => (
          <button
            key={role}
            type="button"
            disabled={switchMode.isPending}
            onClick={() => switchMode.mutate(role)}
            className="flex cursor-pointer flex-col gap-1.5 rounded-lg border border-border bg-muted/40 p-3 text-left transition-all hover:border-primary/40 hover:bg-primary/5 disabled:pointer-events-none disabled:opacity-50"
          >
            <div className="flex items-center gap-2 text-xs font-semibold text-foreground">
              <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
                <Icon className="size-3.5" aria-hidden="true" />
              </span>
              <span className="relay-text-trim leading-none">{t(`sidebar.mode.${role}`)}</span>
            </div>
            <p className="text-xs leading-relaxed text-muted-foreground">{t(descKey)}</p>
          </button>
        ))}
      </div>
      {switchMode.error && (
        <Alert
          variant="destructive"
          role="alert"
          className="border-destructive/30 bg-[var(--relay-stale-soft)]"
        >
          <AlertCircle aria-hidden="true" />
          <AlertDescription>{formatErrorMessage(switchMode.error, t)}</AlertDescription>
        </Alert>
      )}
    </div>
  );
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
    <li className="flex flex-col gap-2 rounded-lg border border-border bg-card p-3 transition-colors">
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 flex-1 items-center gap-2">
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
                size="icon-sm"
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
            size="icon-sm"
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

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/70 pt-2">
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
                size="sm"
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
                size="sm"
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
              size="sm"
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
              size="sm"
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
            size="sm"
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

export function PairDeviceModal({ isOpen, onOpenChange, mode, status }: PairDeviceModalProps) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"share" | "connect">(mode === "client" ? "connect" : "share");
  const [ticket, setTicket] = useState<string>("");
  const [copied, setCopied] = useState(false);
  const [copiedNodeId, setCopiedNodeId] = useState<string | null>(null);
  const [remoteInput, setRemoteInput] = useState("");
  const [busyByNode, setBusyByNode] = useState<Record<string, PeerActionKind>>({});
  const busyNodesRef = useRef(new Set<string>());
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const { data: trustedPeers = [] } = useTrustedPeers();
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

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[88vh] overflow-y-auto overscroll-contain sm:max-w-xl">
        <DialogHeader>
          <div className="flex items-center gap-2.5">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
              <Link2 className="size-4" aria-hidden="true" />
            </div>
            <DialogTitle>{t("modals.pair.title")}</DialogTitle>
          </div>
        </DialogHeader>

        {mode === "local_dual_sandbox" ? (
          <PairingRolePicker />
        ) : (
          <Tabs
            value={tab}
            onValueChange={(value) => setTab(value === "connect" ? "connect" : "share")}
            className="gap-3"
          >
            <TabsList className="grid w-full grid-cols-2">
              <TabsTrigger value="share">
                <span className="relay-text-trim leading-none">{t("modals.pair.shareTab")}</span>
              </TabsTrigger>
              <TabsTrigger value="connect">
                <span className="relay-text-trim leading-none">{t("modals.pair.connectTab")}</span>
              </TabsTrigger>
            </TabsList>

            <TabsContent value="share" className="flex flex-col gap-3">
              <p className="text-xs leading-relaxed text-muted-foreground">
                {t("modals.pair.shareDesc")}
              </p>

              {ticket ? (
                <div className="flex flex-col gap-2">
                  <pre className="font-mono-tech max-h-36 overflow-auto rounded-md border border-border bg-muted/60 p-3 text-[11px] leading-relaxed text-foreground">
                    {ticket}
                  </pre>
                  <div className="flex gap-2">
                    <Button size="lg" className="flex-1" onClick={() => void handleCopy()}>
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
                      size="lg"
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
                <Button
                  size="lg"
                  className="w-full"
                  disabled={getTicket.isPending}
                  onClick={handleGenerate}
                >
                  <Sparkles data-icon="inline-start" aria-hidden="true" />
                  <span className="relay-text-trim leading-none">
                    {t("modals.pair.generateBtn")}
                  </span>
                </Button>
              )}
            </TabsContent>

            <TabsContent value="connect" className="flex flex-col gap-3">
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="pair-code-input">{t("modals.pair.pasteLabel")}</FieldLabel>
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

              <Button
                size="lg"
                className="w-full"
                disabled={!remoteInput.trim() || connectPeer.isPending}
                onClick={handleConnect}
              >
                <Link2 data-icon="inline-start" aria-hidden="true" />
                <span className="relay-text-trim leading-none">{t("modals.pair.connectBtn")}</span>
              </Button>
            </TabsContent>
          </Tabs>
        )}

        <Separator />

        <div className="flex flex-col gap-2">
          <div className="flex items-center gap-2">
            <h3 className="text-xs font-semibold text-foreground">
              {t("modals.pair.trustedTitle")}
            </h3>
            {trustedPeers.length > 0 && (
              <Badge variant="secondary" className="font-mono-tech">
                {trustedPeers.length}
              </Badge>
            )}
          </div>

          {trustedPeers.length === 0 ? (
            <div className="rounded-lg border border-dashed border-border bg-muted/20 px-3 py-4 text-center text-xs leading-relaxed text-muted-foreground">
              {t("modals.pair.trustedEmpty")}
            </div>
          ) : (
            <ul className="flex max-h-60 flex-col gap-2 overflow-y-auto pr-0.5">
              {trustedPeers.map((peer) => {
                const isConnected =
                  peer.revoked_at === null &&
                  Boolean(status?.connected && status.remote_node_id === peer.node_id);
                return (
                  <TrustedDeviceRow
                    key={peer.node_id}
                    peer={peer}
                    mode={mode}
                    isConnected={isConnected}
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
        </div>

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
      </DialogContent>
    </Dialog>
  );
}
