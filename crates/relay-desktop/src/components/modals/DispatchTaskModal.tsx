import { Button, Input, Label, Modal, Switch, TextArea } from "@heroui/react";
import { Play } from "lucide-react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { useTranslation } from "react-i18next";

export interface DispatchTaskFormValues {
  preset: "train_grid" | "smoke";
  windows: string;
  alpha: string;
  userNote: string;
  requireApproval: boolean;
}

interface DispatchTaskModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  headCommitSha: string;
  onDispatch: (values: DispatchTaskFormValues) => void;
  isSubmitting: boolean;
}

export function DispatchTaskModal({
  isOpen,
  onOpenChange,
  headCommitSha,
  onDispatch,
  isSubmitting,
}: DispatchTaskModalProps) {
  const { t } = useTranslation();
  const { register, control, handleSubmit, setValue } = useForm<DispatchTaskFormValues>({
    defaultValues: {
      preset: "train_grid",
      windows: "7,14,28",
      alpha: "0.35",
      userNote: "",
      requireApproval: true,
    },
  });

  const selectedPreset = useWatch({ control, name: "preset" });

  const onSubmit = handleSubmit((values) => {
    onDispatch(values);
    onOpenChange(false);
  });

  return (
    <Modal isOpen={isOpen} onOpenChange={onOpenChange}>
      <Modal.Backdrop variant="blur">
        <Modal.Container placement="center" size="lg">
          <Modal.Dialog className="border border-[var(--relay-border)] bg-[var(--relay-surface)] text-[var(--relay-text)]">
            <Modal.CloseTrigger />
            <Modal.Header>
              <div className="flex items-center gap-2">
                <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-teal-500/12 text-teal-600 dark:text-teal-400">
                  <Play className="h-4 w-4" />
                </div>
                <div>
                  <Modal.Heading className="text-[15px] font-bold">
                    {t("modals.task.title")}
                  </Modal.Heading>
                  <p className="text-[12px] text-[var(--relay-text-muted)]">
                    {t("modals.task.subtitle")} ·{" "}
                    <span className="font-mono-tech">@{headCommitSha.slice(0, 7)}</span>
                  </p>
                </div>
              </div>
            </Modal.Header>

            <form onSubmit={onSubmit}>
              <Modal.Body className="space-y-3.5 py-3">
                {/* Preset Whitelist Picker */}
                <div className="space-y-1.5">
                  <Label className="text-[12px] font-medium">{t("modals.task.presetField")}</Label>
                  <div className="grid grid-cols-2 gap-2">
                    {(["train_grid", "smoke"] as const).map((preset) => {
                      const active = selectedPreset === preset;
                      return (
                        <button
                          key={preset}
                          type="button"
                          onClick={() => setValue("preset", preset)}
                          className={[
                            "rounded-xl border px-3 py-2.5 text-left transition-all cursor-pointer",
                            active
                              ? "border-teal-500 bg-teal-500/8 ring-1 ring-teal-500/25"
                              : "border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] hover:border-[var(--relay-border-strong)]",
                          ].join(" ")}
                        >
                          <div className="font-mono-tech text-[12px] font-semibold text-[var(--relay-text)]">
                            {preset}
                          </div>
                          <div className="mt-0.5 text-[11px] text-[var(--relay-text-muted)]">
                            {preset === "train_grid"
                              ? "uv run forecast.py --windows 7,14,28"
                              : "uv run forecast.py --smoke"}
                          </div>
                        </button>
                      );
                    })}
                  </div>
                </div>

                {selectedPreset === "train_grid" && (
                  <div className="grid grid-cols-2 gap-3">
                    <div className="space-y-1.5">
                      <Label htmlFor="task-windows" className="text-[12px] font-medium">
                        {t("modals.task.windowsField")}
                      </Label>
                      <Input
                        id="task-windows"
                        fullWidth
                        className="font-mono-tech text-[12px]"
                        {...register("windows", { required: true })}
                      />
                    </div>
                    <div className="space-y-1.5">
                      <Label htmlFor="task-alpha" className="text-[12px] font-medium">
                        {t("modals.task.alphaField")}
                      </Label>
                      <Input
                        id="task-alpha"
                        fullWidth
                        className="font-mono-tech text-[12px]"
                        {...register("alpha", { required: true })}
                      />
                    </div>
                  </div>
                )}

                <div className="space-y-1.5">
                  <Label htmlFor="task-note" className="text-[12px] font-medium">
                    {t("modals.task.noteField")}
                  </Label>
                  <TextArea
                    id="task-note"
                    fullWidth
                    rows={2}
                    placeholder={t("modals.task.notePlaceholder")}
                    {...register("userNote")}
                  />
                </div>

                <div className="rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] px-3 py-2.5">
                  <Controller
                    control={control}
                    name="requireApproval"
                    render={({ field }) => (
                      <Switch
                        size="sm"
                        isSelected={field.value}
                        onChange={field.onChange}
                        className="flex items-center justify-between gap-3 w-full"
                      >
                        <Switch.Content className="text-[12px] text-[var(--relay-text)]">
                          {t("modals.task.requireApproval")}
                        </Switch.Content>
                        <Switch.Control>
                          <Switch.Thumb />
                        </Switch.Control>
                      </Switch>
                    )}
                  />
                </div>
              </Modal.Body>
              <Modal.Footer className="gap-2">
                <Button variant="secondary" onPress={() => onOpenChange(false)}>
                  {t("modals.task.cancel")}
                </Button>
                <Button type="submit" variant="primary" isDisabled={isSubmitting}>
                  {t("modals.task.submit")}
                </Button>
              </Modal.Footer>
            </form>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  );
}
