import { useMemo, useState } from "react";
import type { ReactNode } from "react";
import {
  ArrowUp,
  Bot,
  Brain,
  ChevronDown,
  ChevronRight,
  Chrome,
  ImagePlus,
  ListChecks,
  Plug,
  Plus,
  Search,
  Square,
  Target,
  type LucideIcon
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  applyRuntimePreset,
  interruptActiveTurn,
  openThreadGoalEditor,
  sendPrompt,
  setComposerPlanMode,
  setComposerValue,
  setSelectedModel,
  setSelectedThinkingMode
} from "@/stores/workbenchStore";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import type { ModelCapabilities, ModelRef, ProviderModelView, ThinkingMode } from "@/types";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;
type NonDefaultThinkingMode = Exclude<ThinkingMode, "auto">;
type ThinkingModeOption = { value: ThinkingMode | null; label: string; aria: string };
type ComposerVariant = "dock" | "hero";

const defaultThinkingModeOption: ThinkingModeOption = { value: null, label: "Default", aria: "Thinking default" };
const backendThinkingModeOptions: Array<{ value: NonDefaultThinkingMode; label: string; aria: string }> = [
  { value: "off", label: "Off", aria: "Thinking off" },
  { value: "minimal", label: "Minimal", aria: "Thinking minimal" },
  { value: "low", label: "Low", aria: "Thinking low" },
  { value: "medium", label: "Medium", aria: "Thinking medium" },
  { value: "high", label: "High", aria: "Thinking high" },
  { value: "x_high", label: "XHigh", aria: "Thinking xhigh" }
];
const menuControlKeys = new Set(["Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "Tab"]);

const unsupportedCapabilities: ModelCapabilities = {
  supports_tools: false,
  thinking: { supported: false, modes: [] }
};

type ComposerModelOption = {
  key: string;
  providerId: string;
  providerName: string;
  modelId: string;
  label: string;
  detail: string;
  capabilities: ModelCapabilities;
};

export function Composer({ state, variant = "dock" }: { state: WorkbenchState; variant?: ComposerVariant }) {
  const [modelOpen, setModelOpen] = useState(false);
  const [modelSearch, setModelSearch] = useState("");
  const activeSession = state.sessions.find((session) => session.id === state.activeSessionId);
  const busy = activeSession?.status === "running" || activeSession?.status === "awaiting_approval";
  const modelOptions = useMemo(
    () => buildModelOptions(state),
    [state.providerSettings, state.runtimeSettings, state.activeProviderId, state.selectedModel]
  );
  const selectedModelRef = state.selectedModel;
  const selectedOption = selectedModelRef ? modelOptionForRef(modelOptions, selectedModelRef) : null;
  const selectedThinkingMode = normalizedThinkingSelection(state.selectedThinkingMode ?? null);
  const thinkingSupported = selectedOption?.capabilities.thinking.supported ?? false;
  const availableThinkingModes = selectedOption?.capabilities.thinking.modes ?? [];
  const nonDefaultThinkingModes = backendThinkingModeOptions.filter((mode) => availableThinkingModes.includes(mode.value));
  const thinkingMenuOptions: ThinkingModeOption[] = [defaultThinkingModeOption, ...nonDefaultThinkingModes];
  const filteredModelOptions = filterModelOptions(modelOptions, modelSearch);
  const groupedModelOptions = groupModelOptions(filteredModelOptions);

  return (
    <form
      className={cn("composer-shell", variant === "hero" ? "rounded-xl p-3" : "rounded-lg p-2")}
      aria-label="Prompt composer"
      onSubmit={(event) => {
        event.preventDefault();
        if (busy) {
          void interruptActiveTurn();
        } else {
          void sendPrompt();
        }
      }}
    >
      <Textarea
        className={variant === "hero" ? "min-h-[96px] border-transparent bg-transparent px-2 py-1 focus:border-transparent focus:ring-0" : undefined}
        value={state.composerValue}
        onChange={(event) => setComposerValue(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) {
            return;
          }
          event.preventDefault();
          if (busy) {
            void interruptActiveTurn();
          } else {
            void sendPrompt();
          }
        }}
        placeholder={variant === "hero" ? "Ask ExAgent to build, fix, or explain..." : "Message ExAgent"}
        aria-label="Message ExAgent"
      />
      <div className="mt-2 flex items-end justify-between gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-1">
          <ComposerActionsMenu planMode={state.composerPlanMode} canUseGoal={Boolean(state.activeSessionId)} />

          {state.composerPlanMode ? (
            <Button
              type="button"
              variant="secondary"
              className="px-2"
              aria-label="Plan mode enabled"
              onClick={() => setComposerPlanMode(false)}
            >
              <ListChecks className="h-4 w-4" />
              <span>Plan</span>
            </Button>
          ) : null}

          {state.runtimeSettings?.presets.length ? (
            <select
              aria-label="Runtime preset"
              className="type-label-md h-8 max-w-[120px] rounded-md border border-transparent bg-transparent px-2 text-muted outline-none transition-colors hover:bg-surface-2 hover:text-ink focus:ring-2 focus:ring-focus"
              defaultValue=""
              onChange={(event) => {
                if (event.target.value) {
                  applyRuntimePreset(event.target.value);
                }
              }}
            >
              <option value="">Build</option>
              {state.runtimeSettings.presets.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.name}
                </option>
              ))}
            </select>
          ) : null}

          <DropdownMenu open={modelOpen} onOpenChange={setModelOpen}>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                className="max-w-[260px] justify-start px-2 text-muted hover:text-ink"
                aria-label="Composer model"
              >
                <Bot className="h-4 w-4 shrink-0" />
                <span className="truncate">{selectedOption?.label ?? selectedModelRef?.model_id ?? "Choose model"}</span>
                <ChevronDown className="h-3.5 w-3.5 shrink-0 text-subtle" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="start"
              side="top"
              className="max-h-[360px] w-[min(520px,calc(100vw-32px))] overflow-hidden p-2"
            >
              <div className="flex h-10 items-center gap-2 rounded-md bg-surface-1 px-2">
                <Search className="h-4 w-4 shrink-0 text-subtle" />
                <Input
                  aria-label="Search models"
                  className="type-body-md h-8 border-transparent bg-transparent px-0"
                  value={modelSearch}
                  placeholder="Search models"
                  onChange={(event) => setModelSearch(event.target.value)}
                  onKeyDown={(event) => {
                    if (!menuControlKeys.has(event.key)) {
                      event.stopPropagation();
                    }
                  }}
                />
              </div>
              <div className="mt-2 max-h-[292px] overflow-y-auto pr-1">
                {groupedModelOptions.length ? (
                  <DropdownMenuRadioGroup value={selectedOption?.key ?? ""}>
                    {groupedModelOptions.map((group) => (
                      <div key={group.providerId} className="py-1">
                        <div className="type-label-sm px-2 py-1 text-subtle">{group.providerName}</div>
                        {group.options.map((option) => (
                          <DropdownMenuRadioItem
                            key={option.key}
                            value={option.key}
                            className="min-h-10 gap-2 py-2 pr-2"
                            onSelect={() => {
                              setSelectedModel({
                                provider_id: option.providerId,
                                model_id: option.modelId
                              });
                              setModelOpen(false);
                            }}
                          >
                            <div className="min-w-0 flex-1">
                              <div className="type-label-md truncate">{option.label}</div>
                              <div className="type-label-sm truncate text-subtle">{option.detail}</div>
                            </div>
                          </DropdownMenuRadioItem>
                        ))}
                      </div>
                    ))}
                  </DropdownMenuRadioGroup>
                ) : (
                  <div className="type-body-md px-2 py-8 text-center text-subtle">No models found</div>
                )}
              </div>
            </DropdownMenuContent>
          </DropdownMenu>

          {thinkingSupported ? (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button type="button" variant="secondary" className="px-2" aria-label="Thinking mode">
                  <Brain className="h-4 w-4" />
                  {thinkingLabel(selectedThinkingMode)}
                  <ChevronDown className="h-3.5 w-3.5 text-subtle" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" side="top" className="w-44">
                <DropdownMenuRadioGroup value={thinkingRadioValue(selectedThinkingMode)}>
                  {thinkingMenuOptions.map((mode) => (
                    <DropdownMenuRadioItem
                      key={mode.label}
                      value={thinkingRadioValue(mode.value)}
                      aria-label={mode.aria}
                      onSelect={() => setSelectedThinkingMode(mode.value)}
                    >
                      {mode.label}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          <Button
            type={busy ? "button" : "submit"}
            size="icon"
            variant={busy ? "secondary" : "default"}
            className="rounded-full shadow-[0_8px_22px_rgb(0_0_0_/_0.28)]"
            disabled={!busy && !state.composerValue.trim()}
            aria-label={busy ? "Interrupt" : "Send"}
            onClick={busy ? () => void interruptActiveTurn() : undefined}
          >
            {busy ? <Square className="h-3.5 w-3.5" /> : <ArrowUp className="h-4 w-4" />}
          </Button>
        </div>
      </div>
    </form>
  );
}

function ComposerActionsMenu({ planMode, canUseGoal }: { planMode: boolean; canUseGoal: boolean }) {
  const { t } = useI18n();

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button type="button" variant="ghost" size="icon" aria-label="Open composer actions">
          <Plus className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" className="w-72 p-1.5">
        <PlannedActionItem
          icon={ImagePlus}
          label={t("composer.actions.addPhotosAndFiles")}
        />
        <PlannedActionItem
          icon={Chrome}
          label={t("composer.actions.attachChrome")}
        />
        <DropdownMenuSeparator />
        <DropdownMenuCheckboxItem
          checked={planMode}
          onCheckedChange={(checked) => setComposerPlanMode(checked === true)}
          className="min-h-10 gap-3 px-2.5 py-2"
        >
          <ListChecks className="h-4 w-4 shrink-0 text-muted" />
          <span className="type-label-md min-w-0 flex-1 truncate text-ink">
            {t("composer.actions.planMode")}
          </span>
        </DropdownMenuCheckboxItem>
        <DropdownMenuItem
          disabled={!canUseGoal}
          className="min-h-10 gap-3 px-2.5 py-2"
          onSelect={() => openThreadGoalEditor()}
        >
          <Target className="h-4 w-4 shrink-0 text-muted" />
          <span className="type-label-md min-w-0 flex-1 truncate text-ink">
            {t("composer.actions.goal")}
          </span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <PlannedActionItem
          icon={Plug}
          label={t("composer.actions.plugins")}
          trailing={<ChevronRight className="h-4 w-4 text-subtle" />}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function PlannedActionItem({
  icon: Icon,
  label,
  trailing
}: {
  icon: LucideIcon;
  label: string;
  trailing?: ReactNode;
}) {
  return (
    <DropdownMenuItem disabled className="min-h-10 gap-3 px-2.5 py-2">
      <Icon className="h-4 w-4 shrink-0 text-muted" />
      <span className="type-label-md min-w-0 flex-1 truncate text-ink">{label}</span>
      {trailing}
    </DropdownMenuItem>
  );
}

function buildModelOptions(state: WorkbenchState): ComposerModelOption[] {
  const options: ComposerModelOption[] = [];
  const seen = new Set<string>();

  const settings = state.providerSettings;

  if (settings?.model_options.length) {
    settings.model_options.forEach((model) => {
      addModelOptionFromView(options, seen, model, providerNameFor(settings, model.provider_id), detailForModelOption(settings, model));
    });
  } else {
    settings?.providers
      .filter((provider) => provider.supported)
      .forEach((provider) => {
        const active = provider.id === settings.active_provider_id;
        addFallbackModelOption(
          options,
          seen,
          provider.id,
          provider.name,
          active ? settings.config.model : provider.default_model,
          active ? "Configured" : "Provider default"
        );
      });
  }

  state.runtimeSettings?.presets.forEach((preset) => {
    const providerId = state.activeProviderId ?? settings?.active_provider_id ?? "openai";
    addFallbackModelOption(options, seen, providerId, providerNameFor(settings, providerId), preset.model, preset.name);
  });

  const selected = state.selectedModel;
  if (selected) {
    addFallbackModelOption(
      options,
      seen,
      selected.provider_id,
      providerNameFor(settings, selected.provider_id),
      selected.model_id,
      "Selected"
    );
  }

  return options;
}

function addModelOptionFromView(
  options: ComposerModelOption[],
  seen: Set<string>,
  model: ProviderModelView,
  providerName: string,
  detail: string
) {
  const key = `${model.provider_id}:${model.id}`;
  if (seen.has(key)) {
    return;
  }
  seen.add(key);
  options.push({
    key,
    providerId: model.provider_id,
    providerName,
    modelId: model.id,
    label: model.display_name || model.id,
    detail,
    capabilities: model.capabilities
  });
}

function addFallbackModelOption(
  options: ComposerModelOption[],
  seen: Set<string>,
  providerId: string,
  providerName: string,
  modelId: string,
  detail: string
) {
  const normalizedModel = modelId.trim();
  if (!normalizedModel) {
    return;
  }
  const key = `${providerId}:${normalizedModel}`;
  if (seen.has(key)) {
    return;
  }
  seen.add(key);
  options.push({
    key,
    providerId,
    providerName,
    modelId: normalizedModel,
    label: normalizedModel,
    detail,
    capabilities: unsupportedCapabilities
  });
}

function groupModelOptions(options: ComposerModelOption[]) {
  const groups: Array<{ providerId: string; providerName: string; options: ComposerModelOption[] }> = [];
  options.forEach((option) => {
    let group = groups.find((item) => item.providerId === option.providerId);
    if (!group) {
      group = {
        providerId: option.providerId,
        providerName: option.providerName,
        options: []
      };
      groups.push(group);
    }
    group.options.push(option);
  });
  return groups;
}

function filterModelOptions(options: ComposerModelOption[], query: string) {
  const normalized = query.trim().toLowerCase();
  if (!normalized) {
    return options;
  }
  return options.filter((option) =>
    [option.label, option.providerName, option.detail].some((value) => value.toLowerCase().includes(normalized))
  );
}

function modelOptionForRef(options: ComposerModelOption[], model: ModelRef) {
  return (
    options.find((option) => option.providerId === model.provider_id && option.modelId === model.model_id) ?? {
      key: `${model.provider_id}:${model.model_id}`,
      providerId: model.provider_id,
      providerName: model.provider_id,
      modelId: model.model_id,
      label: model.model_id,
      detail: "Selected",
      capabilities: unsupportedCapabilities
    }
  );
}

function detailForModelOption(settings: WorkbenchState["providerSettings"], model: ProviderModelView) {
  if (settings?.config.provider_id === model.provider_id && settings.config.model === model.id) {
    return "Configured";
  }
  const provider = settings?.providers.find((item) => item.id === model.provider_id);
  if (provider?.default_model === model.id) {
    return "Provider default";
  }
  return "Available";
}

function providerNameFor(settings: WorkbenchState["providerSettings"], providerId: string) {
  return settings?.providers.find((provider) => provider.id === providerId)?.name ?? providerId;
}

function normalizedThinkingSelection(mode: ThinkingMode | null): ThinkingMode | null {
  return mode === "auto" ? null : mode;
}

function thinkingLabel(mode: ThinkingMode | null) {
  return [defaultThinkingModeOption, ...backendThinkingModeOptions].find((item) => item.value === mode)?.label ?? "Default";
}

function thinkingRadioValue(mode: ThinkingMode | null) {
  return mode ?? "default";
}
