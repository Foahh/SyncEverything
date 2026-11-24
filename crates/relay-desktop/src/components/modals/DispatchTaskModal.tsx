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
import { Field, FieldGroup, FieldLabel, FieldTitle } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { FolderGit2, Play } from "lucide-react";
import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type { PresetInfo } from "../../bindings";

export interface DispatchTaskFormValues {
  taskId: string;
  preset: string;
  params: Record<string, string>;
  userNote: string;
  requireApproval: boolean;
}

interface DispatchTaskModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  headCommitSha: string | null;
  presets: PresetInfo[];
  onDispatch: (values: DispatchTaskFormValues) => Promise<void>;
  isSubmitting: boolean;
  onOpenRepoSettings?: () => void;
}

function initialParamsFor(preset: PresetInfo): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of preset.allowed_params) {
    result[key] = "";
  }
  return result;
}

function formatCommandPreview(preset: PresetInfo): string {
  return [preset.program, ...preset.args].join(" ");
}

function DispatchTaskForm({
  presets,
  onDispatch,
  onCancel,
  isSubmitting,
}: {
  presets: PresetInfo[];
  onDispatch: (values: DispatchTaskFormValues) => Promise<void>;
  onCancel: () => void;
  isSubmitting: boolean;
}) {
  const { t } = useTranslation();
  const preferred = presets[0];
  const [taskId] = useState(() => crypto.randomUUID());
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [selectedName, setSelectedName] = useState(preferred.name);
  const [params, setParams] = useState<Record<string, string>>(() => initialParamsFor(preferred));
  const [userNote, setUserNote] = useState("");
  const [requireApproval, setRequireApproval] = useState(true);

  const selectedPreset = presets.find((item) => item.name === selectedName) ?? preferred;

  const handleSelectPreset = (preset: PresetInfo) => {
    setSelectedName(preset.name);
    setParams(initialParamsFor(preset));
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setSubmitError(null);
    const cleaned: Record<string, string> = {};
    for (const key of selectedPreset.allowed_params) {
      const value = (params[key] ?? "").trim();
      if (value) {
        cleaned[key] = value;
      }
    }
    try {
      await onDispatch({
        taskId,
        preset: selectedPreset.name,
        params: cleaned,
        userNote,
        requireApproval,
      });
      onCancel();
    } catch (error) {
      setSubmitError(String(error));
    }
  };

  return (
    <form onSubmit={handleSubmit} className="flex flex-col gap-4">
      <FieldGroup>
        <Field>
          <FieldTitle>{t("modals.task.presetLabel")}</FieldTitle>
          <div className="grid grid-cols-2 gap-2">
            {presets.map((preset) => {
              const active = preset.name === selectedPreset.name;
              return (
                <button
                  key={preset.name}
                  type="button"
                  onClick={() => handleSelectPreset(preset)}
                  className={cn(
                    "cursor-pointer rounded-lg border px-3 py-2.5 text-left transition-all",
                    active
                      ? "border-primary bg-primary/8 ring-1 ring-primary/25"
                      : "border-border bg-muted/40 hover:border-foreground/20",
                  )}
                >
                  <div
                    lang="en"
                    className="font-mono-tech truncate text-xs font-semibold text-foreground"
                  >
                    {preset.name}
                  </div>
                  <div className="mt-0.5 line-clamp-2 text-[11px] text-muted-foreground">
                    {preset.description}
                  </div>
                  <div
                    lang="en"
                    className="font-mono-tech mt-1.5 truncate text-[10px] text-muted-foreground/80"
                    title={formatCommandPreview(preset)}
                  >
                    {formatCommandPreview(preset)}
                  </div>
                </button>
              );
            })}
          </div>
        </Field>

        {selectedPreset.allowed_params.length > 0 ? (
          <div className="grid grid-cols-2 gap-3">
            {selectedPreset.allowed_params.map((param) => (
              <Field key={param}>
                <FieldLabel htmlFor={`task-param-${param}`} className="font-mono-tech text-xs">
                  {param}
                </FieldLabel>
                <Input
                  id={`task-param-${param}`}
                  name={`param_${param}`}
                  value={params[param] ?? ""}
                  onChange={(e) =>
                    setParams((prev) => ({
                      ...prev,
                      [param]: e.target.value,
                    }))
                  }
                  placeholder={t("modals.task.paramPlaceholder")}
                  autoCorrect="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  className="font-mono-tech text-xs"
                />
              </Field>
            ))}
          </div>
        ) : (
          <p className="text-[11px] text-muted-foreground">{t("modals.task.noParamsHint")}</p>
        )}

        <Field>
          <FieldLabel htmlFor="task-note">{t("modals.task.noteLabel")}</FieldLabel>
          <Textarea
            id="task-note"
            name="user_note"
            rows={2}
            value={userNote}
            onChange={(e) => setUserNote(e.target.value)}
            placeholder={t("modals.task.notePlaceholder")}
          />
        </Field>

        <Field
          orientation="horizontal"
          className="justify-between rounded-lg border border-border bg-muted/40 px-3 py-2.5"
        >
          <FieldLabel htmlFor="task-ask-before-fix" className="cursor-pointer font-normal">
            {t("modals.task.askBeforeRun")}
          </FieldLabel>
          <Switch
            id="task-ask-before-fix"
            size="sm"
            checked={requireApproval}
            onCheckedChange={setRequireApproval}
          />
        </Field>
      </FieldGroup>

      {submitError && (
        <Alert variant="destructive">
          <AlertDescription>{submitError}</AlertDescription>
        </Alert>
      )}
      <DialogFooter>
        <Button
          type="button"
          variant="outline"
          size="lg"
          disabled={isSubmitting}
          onClick={onCancel}
        >
          <span className="relay-text-trim leading-none">{t("modals.task.cancel")}</span>
        </Button>
        <Button type="submit" size="lg" disabled={isSubmitting}>
          <Play data-icon="inline-start" />
          <span className="relay-text-trim leading-none">{t("modals.task.submit")}</span>
        </Button>
      </DialogFooter>
    </form>
  );
}

export function DispatchTaskModal({
  isOpen,
  onOpenChange,
  headCommitSha,
  presets,
  onDispatch,
  isSubmitting,
  onOpenRepoSettings,
}: DispatchTaskModalProps) {
  const { t } = useTranslation();
  const presetsKey = presets.map((item) => item.name).join(",");
  const commitBadge = headCommitSha ? headCommitSha.slice(0, 7) : "HEAD";

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!isSubmitting) onOpenChange(open);
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <div className="flex items-center gap-2.5">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
              <Play className="size-4" />
            </div>
            <div className="flex flex-col gap-0.5">
              <DialogTitle>{t("modals.task.title")}</DialogTitle>
              <DialogDescription>
                {t("modals.task.subtitle")} · <span className="font-mono-tech">{commitBadge}</span>
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        {presets.length > 0 ? (
          <DispatchTaskForm
            key={presetsKey}
            presets={presets}
            onDispatch={onDispatch}
            onCancel={() => onOpenChange(false)}
            isSubmitting={isSubmitting}
          />
        ) : (
          <div className="flex flex-col gap-4">
            <div className="rounded-lg border border-dashed border-border bg-muted/30 p-4 text-center">
              <div className="text-xs font-semibold text-foreground">
                {t("modals.task.noPresetsTitle")}
              </div>
              <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                {t("modals.task.noPresetsDesc")}
              </p>
              {onOpenRepoSettings && (
                <Button
                  type="button"
                  variant="outline"
                  size="default"
                  className="mt-3"
                  onClick={() => {
                    onOpenChange(false);
                    onOpenRepoSettings();
                  }}
                >
                  <FolderGit2 data-icon="inline-start" />
                  <span className="relay-text-trim leading-none">
                    {t("modals.task.openRepoSettings")}
                  </span>
                </Button>
              )}
            </div>

            <DialogFooter>
              <Button type="button" variant="outline" size="lg" onClick={() => onOpenChange(false)}>
                <span className="relay-text-trim leading-none">{t("modals.task.cancel")}</span>
              </Button>
              <Button type="button" size="lg" disabled>
                <Play data-icon="inline-start" />
                <span className="relay-text-trim leading-none">{t("modals.task.submit")}</span>
              </Button>
            </DialogFooter>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
