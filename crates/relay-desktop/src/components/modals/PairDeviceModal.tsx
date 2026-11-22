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
import { AlertCircle, Check, CheckCircle2, Copy, Link2, Sparkles } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectPeerTicket, useGetPairingTicket } from "../../lib/queries";

interface PairDeviceModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
}

export function PairDeviceModal({ isOpen, onOpenChange }: PairDeviceModalProps) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"share" | "connect">("share");
  const [ticket, setTicket] = useState<string>("");
  const [copied, setCopied] = useState(false);
  const [remoteInput, setRemoteInput] = useState("");
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const getTicket = useGetPairingTicket();
  const connectPeer = useConnectPeerTicket();

  const handleGenerate = () => {
    setCopied(false);
    getTicket.mutate(undefined, {
      onSuccess: (generated) => setTicket(generated),
    });
  };

  const handleCopy = async () => {
    if (!ticket) return;
    await navigator.clipboard.writeText(ticket);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1800);
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
      onError: () => {
        setFeedback({ ok: false, text: t("modals.pair.invalidCode") });
      },
    });
  };

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <div className="flex items-center gap-2.5">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <Link2 className="size-4" />
            </div>
            <div className="flex flex-col gap-0.5">
              <DialogTitle>{t("modals.pair.title")}</DialogTitle>
              <DialogDescription>{t("modals.pair.subtitle")}</DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <Tabs
          value={tab}
          onValueChange={(value) => setTab(value === "connect" ? "connect" : "share")}
          className="gap-3"
        >
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="share">{t("modals.pair.shareTab")}</TabsTrigger>
            <TabsTrigger value="connect">{t("modals.pair.connectTab")}</TabsTrigger>
          </TabsList>

          <TabsContent value="share" className="flex flex-col gap-3">
            <p className="text-xs leading-relaxed text-muted-foreground">
              {t("modals.pair.shareDesc")}
            </p>

            {ticket ? (
              <div className="flex flex-col gap-2">
                <pre className="font-mono-tech max-h-40 overflow-auto rounded-lg border border-border bg-muted/60 p-3 text-[11px] leading-relaxed text-foreground">
                  {ticket}
                </pre>
                <div className="flex gap-2">
                  <Button size="lg" className="flex-1" onClick={() => void handleCopy()}>
                    {copied ? (
                      <Check data-icon="inline-start" />
                    ) : (
                      <Copy data-icon="inline-start" />
                    )}
                    {copied ? t("modals.pair.copiedBtn") : t("modals.pair.copyBtn")}
                  </Button>
                  <Button
                    size="lg"
                    variant="outline"
                    disabled={getTicket.isPending}
                    onClick={handleGenerate}
                  >
                    {t("modals.pair.generateBtn")}
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
                {t("modals.pair.generateBtn")}
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
                  className="font-mono-tech text-[11.5px]"
                />
              </Field>
            </FieldGroup>

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

            <Button
              size="lg"
              className="w-full"
              disabled={!remoteInput.trim() || connectPeer.isPending}
              onClick={handleConnect}
            >
              <Link2 data-icon="inline-start" />
              {t("modals.pair.connectBtn")}
            </Button>
          </TabsContent>
        </Tabs>

        <DialogFooter>
          <Button variant="outline" size="lg" onClick={() => onOpenChange(false)}>
            {t("modals.pair.close")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
