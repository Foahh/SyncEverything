import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldGroup, FieldLabel, FieldTitle } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
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
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <div className="flex items-center gap-2.5">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <Play className="size-4" />
            </div>
            <div className="flex flex-col gap-0.5">
              <DialogTitle>{t("modals.task.title")}</DialogTitle>
              <DialogDescription>
                {t("modals.task.subtitle")} ·{" "}
                <span className="font-mono-tech">{headCommitSha.slice(0, 7)}</span>
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <form onSubmit={onSubmit} className="flex flex-col gap-4">
          <FieldGroup>
            <Field>
              <FieldTitle>{t("modals.task.presetLabel")}</FieldTitle>
              <div className="grid grid-cols-2 gap-2">
                {(["train_grid", "smoke"] as const).map((preset) => {
                  const active = selectedPreset === preset;
                  return (
                    <button
                      key={preset}
                      type="button"
                      onClick={() => setValue("preset", preset)}
                      className={cn(
                        "cursor-pointer rounded-xl border px-3 py-2.5 text-left transition-all",
                        active
                          ? "border-primary bg-primary/8 ring-1 ring-primary/25"
                          : "border-border bg-muted/40 hover:border-foreground/20",
                      )}
                    >
                      <div className="text-xs font-semibold text-foreground">
                        {preset === "train_grid"
                          ? t("modals.task.presetGridTitle")
                          : t("modals.task.presetSmokeTitle")}
                      </div>
                      <div className="mt-0.5 text-[11px] text-muted-foreground">
                        {preset === "train_grid"
                          ? t("modals.task.presetGridDesc")
                          : t("modals.task.presetSmokeDesc")}
                      </div>
                    </button>
                  );
                })}
              </div>
            </Field>

            {selectedPreset === "train_grid" && (
              <div className="grid grid-cols-2 gap-3">
                <Field>
                  <FieldLabel htmlFor="task-windows">{t("modals.task.windowsLabel")}</FieldLabel>
                  <Input
                    id="task-windows"
                    className="font-mono-tech text-xs"
                    {...register("windows", { required: true })}
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="task-alpha">{t("modals.task.alphaLabel")}</FieldLabel>
                  <Input
                    id="task-alpha"
                    className="font-mono-tech text-xs"
                    {...register("alpha", { required: true })}
                  />
                </Field>
              </div>
            )}

            <Field>
              <FieldLabel htmlFor="task-note">{t("modals.task.noteLabel")}</FieldLabel>
              <Textarea
                id="task-note"
                rows={2}
                placeholder={t("modals.task.notePlaceholder")}
                {...register("userNote")}
              />
            </Field>

            <Field
              orientation="horizontal"
              className="justify-between rounded-xl border border-border bg-muted/40 px-3 py-2.5"
            >
              <FieldLabel htmlFor="task-ask-before-fix" className="cursor-pointer font-normal">
                {t("modals.task.askBeforeFix")}
              </FieldLabel>
              <Controller
                control={control}
                name="requireApproval"
                render={({ field }) => (
                  <Switch
                    id="task-ask-before-fix"
                    size="sm"
                    checked={field.value}
                    onCheckedChange={field.onChange}
                  />
                )}
              />
            </Field>
          </FieldGroup>

          <DialogFooter>
            <Button type="button" variant="outline" size="lg" onClick={() => onOpenChange(false)}>
              {t("modals.task.cancel")}
            </Button>
            <Button type="submit" size="lg" disabled={isSubmitting}>
              <Play data-icon="inline-start" />
              {t("modals.task.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
