import { Button, Input, Label, Modal, TextArea } from "@heroui/react";
import { StickyNote } from "lucide-react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";

export interface RecordDecisionFormValues {
  label: string;
  rationale: string;
  constraintsRaw: string;
}

interface RecordDecisionModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  onSubmitDecision: (values: { label: string; rationale: string; constraints: string[] }) => void;
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
    formState: { isValid },
  } = useForm<RecordDecisionFormValues>({
    mode: "onChange",
    defaultValues: {
      label: "",
      rationale: "",
      constraintsRaw: "",
    },
  });

  const onSubmit = handleSubmit((data) => {
    const constraints = data.constraintsRaw
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    onSubmitDecision({
      label: data.label.trim(),
      rationale: data.rationale.trim(),
      constraints,
    });
    reset();
    onOpenChange(false);
  });

  return (
    <Modal isOpen={isOpen} onOpenChange={onOpenChange}>
      <Modal.Backdrop variant="blur">
        <Modal.Container placement="center" size="lg">
          <Modal.Dialog className="border border-[var(--relay-border)] bg-[var(--relay-surface)] text-[var(--relay-text)]">
            <Modal.CloseTrigger />
            <Modal.Header>
              <div className="flex items-center gap-2.5">
                <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-indigo-500/12 text-indigo-600 dark:text-indigo-400">
                  <StickyNote className="h-4 w-4" />
                </div>
                <div>
                  <Modal.Heading className="text-[15px] font-semibold">
                    {t("modals.note.title")}
                  </Modal.Heading>
                  <p className="text-[12px] text-[var(--relay-text-muted)]">
                    {t("modals.note.subtitle")}
                  </p>
                </div>
              </div>
            </Modal.Header>
            <form onSubmit={onSubmit}>
              <Modal.Body className="space-y-3.5 py-3">
                <div className="space-y-1.5">
                  <Label htmlFor="note-label" isRequired className="text-[12px] font-medium">
                    {t("modals.note.titleLabel")}
                  </Label>
                  <Input
                    id="note-label"
                    fullWidth
                    placeholder={t("modals.note.titlePlaceholder")}
                    {...register("label", { required: true, minLength: 2 })}
                  />
                </div>

                <div className="space-y-1.5">
                  <Label htmlFor="note-why" isRequired className="text-[12px] font-medium">
                    {t("modals.note.whyLabel")}
                  </Label>
                  <TextArea
                    id="note-why"
                    fullWidth
                    rows={3}
                    placeholder={t("modals.note.whyPlaceholder")}
                    {...register("rationale", { required: true, minLength: 3 })}
                  />
                </div>

                <div className="space-y-1.5">
                  <Label htmlFor="note-rules" className="text-[12px] font-medium">
                    {t("modals.note.rulesLabel")}
                  </Label>
                  <Input
                    id="note-rules"
                    fullWidth
                    placeholder={t("modals.note.rulesPlaceholder")}
                    {...register("constraintsRaw")}
                  />
                </div>
              </Modal.Body>
              <Modal.Footer className="gap-2">
                <Button variant="secondary" onPress={() => onOpenChange(false)}>
                  {t("modals.note.cancel")}
                </Button>
                <Button type="submit" variant="primary" isDisabled={!isValid || isSubmitting}>
                  {t("modals.note.submit")}
                </Button>
              </Modal.Footer>
            </form>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  );
}
