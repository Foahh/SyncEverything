import { Button, Label, Modal, Tabs, TextArea } from "@heroui/react";
import { Check, Copy, Link2, Sparkles } from "lucide-react";
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
    <Modal isOpen={isOpen} onOpenChange={onOpenChange}>
      <Modal.Backdrop variant="blur">
        <Modal.Container placement="center" size="lg">
          <Modal.Dialog className="border border-[var(--relay-border)] bg-[var(--relay-surface)] text-[var(--relay-text)]">
            <Modal.CloseTrigger />
            <Modal.Header>
              <div className="flex items-center gap-2.5">
                <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-teal-500/12 text-teal-600 dark:text-teal-400">
                  <Link2 className="h-4 w-4" />
                </div>
                <div>
                  <Modal.Heading className="text-[15px] font-semibold">
                    {t("modals.pair.title")}
                  </Modal.Heading>
                  <p className="text-[12px] text-[var(--relay-text-muted)]">
                    {t("modals.pair.subtitle")}
                  </p>
                </div>
              </div>
            </Modal.Header>

            <Modal.Body className="space-y-4 py-3">
              <Tabs
                selectedKey={tab}
                onSelectionChange={(k) => setTab(k === "connect" ? "connect" : "share")}
              >
                <Tabs.ListContainer className="w-full overflow-visible">
                  <Tabs.List className="grid w-full grid-cols-2 rounded-lg bg-[var(--relay-surface-subtle)] p-0.5">
                    <Tabs.Tab
                      id="share"
                      className="whitespace-nowrap py-1.5 text-[12px] font-medium"
                    >
                      <Tabs.Indicator />
                      {t("modals.pair.shareTab")}
                    </Tabs.Tab>
                    <Tabs.Tab
                      id="connect"
                      className="whitespace-nowrap py-1.5 text-[12px] font-medium"
                    >
                      <Tabs.Indicator />
                      {t("modals.pair.connectTab")}
                    </Tabs.Tab>
                  </Tabs.List>
                </Tabs.ListContainer>
              </Tabs>

              {tab === "share" ? (
                <div className="space-y-3">
                  <p className="text-[12.5px] leading-relaxed text-[var(--relay-text-muted)]">
                    {t("modals.pair.shareDesc")}
                  </p>

                  {ticket ? (
                    <div className="space-y-2">
                      <pre className="font-mono-tech max-h-40 overflow-auto rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] p-3 text-[11px] leading-relaxed text-[var(--relay-text)]">
                        {ticket}
                      </pre>
                      <div className="flex gap-2">
                        <Button
                          size="sm"
                          variant="primary"
                          className="flex-1"
                          onPress={() => void handleCopy()}
                        >
                          {copied ? (
                            <Check className="h-3.5 w-3.5" />
                          ) : (
                            <Copy className="h-3.5 w-3.5" />
                          )}
                          {copied ? t("modals.pair.copiedBtn") : t("modals.pair.copyBtn")}
                        </Button>
                        <Button
                          size="sm"
                          variant="secondary"
                          isDisabled={getTicket.isPending}
                          onPress={handleGenerate}
                        >
                          {t("modals.pair.generateBtn")}
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <Button
                      variant="primary"
                      fullWidth
                      isDisabled={getTicket.isPending}
                      onPress={handleGenerate}
                    >
                      <Sparkles className="h-4 w-4" />
                      {t("modals.pair.generateBtn")}
                    </Button>
                  )}
                </div>
              ) : (
                <div className="space-y-3">
                  <div className="space-y-1.5">
                    <Label htmlFor="pair-code-input" className="text-[12px] font-medium">
                      {t("modals.pair.pasteLabel")}
                    </Label>
                    <TextArea
                      id="pair-code-input"
                      fullWidth
                      rows={4}
                      value={remoteInput}
                      onChange={(e) => setRemoteInput(e.target.value)}
                      placeholder={t("modals.pair.pastePlaceholder")}
                      className="font-mono-tech text-[11.5px]"
                    />
                  </div>

                  {feedback && (
                    <div
                      className={[
                        "rounded-lg px-3 py-2 text-[12px] font-medium",
                        feedback.ok
                          ? "bg-[var(--relay-fresh-soft)] text-teal-800 dark:text-teal-200"
                          : "bg-[var(--relay-stale-soft)] text-red-700 dark:text-red-300",
                      ].join(" ")}
                    >
                      {feedback.text}
                    </div>
                  )}

                  <Button
                    variant="primary"
                    fullWidth
                    isDisabled={!remoteInput.trim() || connectPeer.isPending}
                    onPress={handleConnect}
                  >
                    <Link2 className="h-4 w-4" />
                    {t("modals.pair.connectBtn")}
                  </Button>
                </div>
              )}
            </Modal.Body>

            <Modal.Footer>
              <Button variant="secondary" onPress={() => onOpenChange(false)}>
                {t("modals.pair.close")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  );
}
