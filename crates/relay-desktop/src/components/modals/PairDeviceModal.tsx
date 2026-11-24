import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { AlertCircle, Check, CheckCircle2, Copy, Cpu, Laptop, Link2, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { RuntimeMode } from "../../bindings";
import { formatErrorMessage } from "../../lib/formatError";
import { useConnectPeerTicket, useGetPairingTicket, useSwitchRuntimeMode } from "../../lib/queries";

interface PairDeviceModalProps {
  mode: RuntimeMode;
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
                <Icon className="size-3.5" />
              </span>
              <span className="relay-text-trim leading-none">{t(`sidebar.mode.${role}`)}</span>
            </div>
            <p className="text-xs leading-relaxed text-muted-foreground">{t(descKey)}</p>
          </button>
        ))}
      </div>
      {switchMode.error && (
        <Alert variant="destructive" className="border-destructive/30 bg-[var(--relay-stale-soft)]">
          <AlertCircle />
          <AlertDescription>{formatErrorMessage(switchMode.error, t)}</AlertDescription>
        </Alert>
      )}
    </div>
  );
}

export function PairDeviceModal({ isOpen, onOpenChange, mode }: PairDeviceModalProps) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"share" | "connect">(mode === "client" ? "connect" : "share");
  const [ticket, setTicket] = useState<string>("");
  const [copied, setCopied] = useState(false);
  const [remoteInput, setRemoteInput] = useState("");
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const getTicket = useGetPairingTicket();
  const connectPeer = useConnectPeerTicket();

  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 1800);
    return () => window.clearTimeout(id);
  }, [copied]);

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

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <div className="flex items-center gap-2.5">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
              <Link2 className="size-4" />
            </div>
            <div className="flex flex-col gap-0.5">
              <DialogTitle>{t("modals.pair.title")}</DialogTitle>
              <DialogDescription>{t("modals.pair.subtitle")}</DialogDescription>
            </div>
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
                  <pre className="font-mono-tech max-h-40 overflow-auto rounded-md border border-border bg-muted/60 p-3 text-[11px] leading-relaxed text-foreground">
                    {ticket}
                  </pre>
                  <div className="flex gap-2">
                    <Button size="lg" className="flex-1" onClick={() => void handleCopy()}>
                      {copied ? (
                        <Check data-icon="inline-start" />
                      ) : (
                        <Copy data-icon="inline-start" />
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
                  <Sparkles data-icon="inline-start" />
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
                    rows={4}
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
                <Link2 data-icon="inline-start" />
                <span className="relay-text-trim leading-none">{t("modals.pair.connectBtn")}</span>
              </Button>
            </TabsContent>
          </Tabs>
        )}
        {feedback && (
          <Alert
            variant={feedback.ok ? "default" : "destructive"}
            className={
              feedback.ok
                ? "border-emerald-500/25 bg-[var(--relay-fresh-soft)] text-emerald-800 dark:text-emerald-200"
                : "border-destructive/30 bg-[var(--relay-stale-soft)]"
            }
          >
            {feedback.ok ? (
              <CheckCircle2 className="text-emerald-600 dark:text-emerald-400" />
            ) : (
              <AlertCircle />
            )}
            <AlertDescription
              className={feedback.ok ? "text-emerald-800 dark:text-emerald-200" : undefined}
            >
              {feedback.text}
            </AlertDescription>
          </Alert>
        )}

        <DialogFooter>
          <Button variant="outline" size="lg" onClick={() => onOpenChange(false)}>
            <span className="relay-text-trim leading-none">{t("modals.pair.close")}</span>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
