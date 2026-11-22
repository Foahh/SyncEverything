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
import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type { PresetInfo } from "../../bindings";

export interface DispatchTaskFormValues {
  preset: string;
  params: Record<string, string>;
  userNote: string;
  requireApproval: boolean;
}

interface DispatchTaskModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  headCommitSha: string;
  presets: PresetInfo[];
  onDispatch: (values: DispatchTaskFormValues) => void;
  isSubmitting: boolean;
}

const DEFAULT_PARAM_VALUES: Record<string, string> = {
  windows: "7,14,28",
  alpha: "0.35",
  window: "7",
};

const FALLBACK_PRESETS: PresetInfo[] = [
  {
    name: "train_grid",
    description: "Run rolling windows (7, 14, 28) and record MAE",
    program: "uv",
    args: [],
    allowed_params: ["windows", "alpha"],
  },
  {
    name: "smoke",
    description: "Fast 1-second environment & script check",
    program: "uv",
    args: [],
    allowed_params: [],
  },
];

function initialParamsFor(preset: PresetInfo): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of preset.allowed_params) {
    result[key] = DEFAULT_PARAM_VALUES[key] ?? "";
  }
  return result;
}

function DispatchTaskForm({
  presets,
  onDispatch,
  onCancel,
  isSubmitting,
}: {
  presets: PresetInfo[];
  onDispatch: (values: DispatchTaskFormValues) => void;
  onCancel: () => void;
  isSubmitting: boolean;
}) {
  const { t } = useTranslation();
  const preferred =
    presets.find((item) => item.name === "train_grid") ?? presets[0] ?? FALLBACK_PRESETS[0];
  const [selectedName, setSelectedName] = useState(preferred.name);
  const [params, setParams] = useState<Record<string, string>>(() => initialParamsFor(preferred));
  const [userNote, setUserNote] = useState("");
  const [requireApproval, setRequireApproval] = useState(true);

  const selectedPreset = presets.find((item) => item.name === selectedName) ?? preferred;

  const handleSelectPreset = (preset: PresetInfo) => {
    setSelectedName(preset.name);
    setParams(initialParamsFor(preset));
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const cleaned: Record<string, string> = {};
    for (const key of selectedPreset.allowed_params) {
      const value = (params[key] ?? "").trim();
      if (value) {
        cleaned[key] = value;
      }
    }
    onDispatch({
      preset: selectedPreset.name,
      params: cleaned,
      userNote,
      requireApproval,
    });
    onCancel();
  };

  return (
    <form onSubmit={handleSubmit} className="flex flex-col gap-4">
      <FieldGroup>
        <Field>
          <FieldTitle>{t("modals.task.presetLabel")}</FieldTitle>
          <div className="grid grid-cols-2 gap-2">
            {presets.map((preset) => {
              const active = preset.name === selectedPreset.name;
              const title =
                preset.name === "train_grid"
                  ? t("modals.task.presetGridTitle")
                  : preset.name === "smoke"
                    ? t("modals.task.presetSmokeTitle")
                    : preset.name;
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
                  <div className="truncate text-xs font-semibold text-foreground">{title}</div>
                  <div className="mt-0.5 line-clamp-2 text-[11px] text-muted-foreground">
                    {preset.description}
                  </div>
                </button>
              );
            })}
          </div>
        </Field>

        {selectedPreset.allowed_params.length > 0 ? (
          <div className="grid grid-cols-2 gap-3">
            {selectedPreset.allowed_params.map((param) => {
              const label =
                param === "windows"
                  ? t("modals.task.windowsLabel")
                  : param === "alpha"
                    ? t("modals.task.alphaLabel")
                    : param;
              return (
                <Field key={param}>
                  <FieldLabel htmlFor={`task-param-${param}`}>{label}</FieldLabel>
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
                    autoCorrect="off"
                    autoCapitalize="none"
                    spellCheck={false}
                    className="font-mono-tech text-xs"
                  />
                </Field>
              );
            })}
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
            {t("modals.task.askBeforeFix")}
          </FieldLabel>
          <Switch
            id="task-ask-before-fix"
            size="sm"
            checked={requireApproval}
            onCheckedChange={setRequireApproval}
          />
        </Field>
      </FieldGroup>

      <DialogFooter>
        <Button type="button" variant="outline" size="lg" onClick={onCancel}>
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
}: DispatchTaskModalProps) {
  const { t } = useTranslation();
  const effectivePresets = presets.length > 0 ? presets : FALLBACK_PRESETS;
  const presetsKey = effectivePresets.map((item) => item.name).join(",");

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <div className="flex items-center gap-2.5">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
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

        <DispatchTaskForm
          key={presetsKey}
          presets={effectivePresets}
          onDispatch={onDispatch}
          onCancel={() => onOpenChange(false)}
          isSubmitting={isSubmitting}
        />
      </DialogContent>
    </Dialog>
  );
}
