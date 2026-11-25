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
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { AlertCircle, StickyNote } from "lucide-react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { formatErrorMessage } from "../../lib/formatError";
import { isValidNoteLabel, parseNoteConstraints } from "../../lib/noteForm";

export interface RecordDecisionFormValues {
  label: string;
  rationale: string;
  constraintsRaw: string;
}

interface RecordDecisionModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  onSubmitDecision: (values: {
    label: string;
    rationale: string;
    constraints: string[];
  }) => Promise<void>;
  isSubmitting: boolean;
}

export function RecordDecisionModal({
  isOpen,
  onOpenChange,
  onSubmitDecision,
  isSubmitting,
}: RecordDecisionModalProps) {
  const { t } = useTranslation();
  const {
    register,
    handleSubmit,
    reset,
    setError,
    formState: { errors },
  } = useForm<RecordDecisionFormValues>({
    mode: "onChange",
    defaultValues: {
      label: "",
      rationale: "",
      constraintsRaw: "",
    },
  });

  const onSubmit = handleSubmit(async (data) => {
    const constraints = parseNoteConstraints(data.constraintsRaw);
    try {
      await onSubmitDecision({
        label: data.label.trim(),
        rationale: data.rationale.trim(),
        constraints,
      });
      reset();
      onOpenChange(false);
    } catch (error) {
      setError("root", { message: formatErrorMessage(error, t) });
    }
  });

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <div className="flex items-center gap-2.5">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
              <StickyNote className="size-4" />
            </div>
            <div className="flex flex-col gap-0.5">
              <DialogTitle>{t("modals.note.title")}</DialogTitle>
              <DialogDescription>{t("modals.note.subtitle")}</DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <form onSubmit={onSubmit} className="flex flex-col gap-4" noValidate>
          <FieldGroup>
            <Field data-invalid={!!errors.label}>
              <FieldLabel htmlFor="note-label">
                {t("modals.note.titleLabel")}
                <span className="text-destructive">*</span>
              </FieldLabel>
              <Input
                id="note-label"
                aria-invalid={!!errors.label}
                placeholder={t("modals.note.titlePlaceholder")}
                {...register("label", {
                  validate: (value) => isValidNoteLabel(value) || t("modals.note.titleRequired"),
                })}
              />
              <FieldError errors={[errors.label]} />
            </Field>

            <Field>
              <FieldLabel htmlFor="note-why">{t("modals.note.whyLabel")}</FieldLabel>
              <Textarea
                id="note-why"
                rows={3}
                placeholder={t("modals.note.whyPlaceholder")}
                {...register("rationale")}
              />
            </Field>

            <Field>
              <FieldLabel htmlFor="note-rules">{t("modals.note.rulesLabel")}</FieldLabel>
              <Input
                id="note-rules"
                placeholder={t("modals.note.rulesPlaceholder")}
                {...register("constraintsRaw")}
              />
            </Field>
          </FieldGroup>

          {errors.root?.message && (
            <Alert
              variant="destructive"
              className="border-destructive/30 bg-[var(--relay-stale-soft)]"
            >
              <AlertCircle />
              <AlertDescription>{errors.root.message}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" size="lg" onClick={() => onOpenChange(false)}>
              <span className="relay-text-trim leading-none">{t("modals.note.cancel")}</span>
            </Button>
            <Button type="submit" size="lg" disabled={isSubmitting}>
              <span className="relay-text-trim leading-none">{t("modals.note.submit")}</span>
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
