import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  ArrowUp,
  Bot,
  Brain,
  ChevronDown,
  ChevronRight,
  Chrome,
  CircleAlert,
  Database,
  ImagePlus,
  ListChecks,
  Plug,
  Plus,
  Search,
  Square,
  Target,
  X,
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
  addComposerAttachments,
  compactActiveThread,
  interruptActiveTurn,
  openThreadGoalEditor,
  removeComposerAttachment,
  sendPrompt,
  setComposerPlanMode,
  setComposerValue,
  setSelectedModel,
  setSelectedThinkingMode
} from "@/stores/workbenchStore";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import { exagentClient } from "@/api/exagentClient";
import type { ComposerAttachment, ModelCapabilities, ModelRef, ProviderModelView, ThinkingMode } from "@/types";
import { useI18n } from "@/lib/i18n";
import { localFileAssetSrc } from "@/lib/media";
import { cn } from "@/lib/utils";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;
type NonDefaultThinkingMode = Exclude<ThinkingMode, "auto">;
type ThinkingModeOption = { value: ThinkingMode | null; label: string; aria: string };
type ComposerVariant = "dock" | "hero";
type SlashCommandId = "compact" | "goal" | "plan";

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

type ImageDragDropHandlers = Parameters<typeof exagentClient.subscribeImageDragDrop>[0];
type DragDropComposerRegistration = { owner: symbol; handlers: ImageDragDropHandlers };

const dragDropNativeSubscriptionRetryMs = 1000;
const dragDropComposerRegistrations: DragDropComposerRegistration[] = [];
let dragDropNativeUnlisten: (() => void) | null = null;
let dragDropNativeSubscriptionPending = false;
let dragDropNativeSubscriptionRetry: ReturnType<typeof setTimeout> | null = null;

function registerDragDropComposer(owner: symbol, handlers: ImageDragDropHandlers) {
  dragDropComposerRegistrations.push({ owner, handlers });
  ensureDragDropNativeSubscription();
}

function unregisterDragDropComposer(owner: symbol) {
  for (let index = dragDropComposerRegistrations.length - 1; index >= 0; index -= 1) {
    if (dragDropComposerRegistrations[index]?.owner === owner) {
      dragDropComposerRegistrations.splice(index, 1);
      break;
    }
  }
  if (dragDropComposerRegistrations.length === 0 && dragDropNativeUnlisten) {
    dragDropNativeUnlisten();
    dragDropNativeUnlisten = null;
  }
  if (dragDropComposerRegistrations.length === 0) {
    clearDragDropNativeSubscriptionRetry();
  }
}

function ensureDragDropNativeSubscription() {
  if (
    dragDropNativeUnlisten ||
    dragDropNativeSubscriptionPending ||
    dragDropNativeSubscriptionRetry ||
    dragDropComposerRegistrations.length === 0
  ) {
    return;
  }

  dragDropNativeSubscriptionPending = true;
  void exagentClient
    .subscribeImageDragDrop({
      onEnter: (paths) => activeDragDropComposerRegistration()?.handlers.onEnter(paths),
      onLeave: () => activeDragDropComposerRegistration()?.handlers.onLeave(),
      onDrop: (paths) => activeDragDropComposerRegistration()?.handlers.onDrop(paths)
    })
    .then((unlisten) => {
      dragDropNativeSubscriptionPending = false;
      clearDragDropNativeSubscriptionRetry();
      if (dragDropComposerRegistrations.length === 0) {
        unlisten();
      } else {
        dragDropNativeUnlisten = unlisten;
      }
    })
    .catch(() => {
      dragDropNativeSubscriptionPending = false;
      dragDropNativeUnlisten = null;
      if (dragDropComposerRegistrations.length > 0) {
        scheduleDragDropNativeSubscriptionRetry();
      }
    });
}

function scheduleDragDropNativeSubscriptionRetry() {
  if (dragDropNativeSubscriptionRetry || dragDropComposerRegistrations.length === 0) {
    return;
  }
  dragDropNativeSubscriptionRetry = setTimeout(() => {
    dragDropNativeSubscriptionRetry = null;
    ensureDragDropNativeSubscription();
  }, dragDropNativeSubscriptionRetryMs);
}

function clearDragDropNativeSubscriptionRetry() {
  if (!dragDropNativeSubscriptionRetry) {
    return;
  }
  clearTimeout(dragDropNativeSubscriptionRetry);
  dragDropNativeSubscriptionRetry = null;
}

function activeDragDropComposerRegistration() {
  return dragDropComposerRegistrations.at(-1);
}

function modelSupportsImages(capabilities: ModelCapabilities | undefined, model: ModelRef | null) {
  if (capabilities?.input_modalities?.length) {
    return capabilities.input_modalities.includes("image");
  }
  if (!model) {
    return true;
  }
  return !isKnownTextOnlyModel(model);
}

function isKnownTextOnlyModel(model: ModelRef) {
  const providerId = model.provider_id.toLowerCase();
  const modelId = model.model_id.toLowerCase();
  return providerId === "deepseek" || modelId.startsWith("embedding") || modelId.includes("/embedding");
}

type ComposerModelOption = {
  key: string;
  providerId: string;
  providerName: string;
  modelId: string;
  label: string;
  detail: string;
  capabilities: ModelCapabilities;
};

type SlashCommand = {
  id: SlashCommandId;
  command: string;
  label: string;
  detail: string;
  icon: LucideIcon;
  disabled: boolean;
};

export function Composer({ state, variant = "dock" }: { state: WorkbenchState; variant?: ComposerVariant }) {
  const { t } = useI18n();
  const [modelOpen, setModelOpen] = useState(false);
  const [modelSearch, setModelSearch] = useState("");
  const [slashDismissedValue, setSlashDismissedValue] = useState<string | null>(null);
  const [selectedSlashCommandIndex, setSelectedSlashCommandIndex] = useState(0);
  const slashMenuId = useId();
  const [draggingImages, setDraggingImages] = useState(false);
  const [imageAttachRejected, setImageAttachRejected] = useState(false);
  const [imageAttachError, setImageAttachError] = useState<string | null>(null);
  const activeSession = state.sessions.find((session) => session.id === state.activeSessionId);
  const busy = activeSession?.status === "running" || activeSession?.status === "awaiting_approval";
  const providerConfigurationRequired = isProviderConfigurationRequired(state.providerSettings);
  const modelOptions = useMemo(
    () => buildModelOptions(state),
    [state.providerSettings, state.runtimeSettings, state.activeProviderId, state.selectedModel]
  );
  const selectedModelRef = state.selectedModel;
  const selectedOption =
    !providerConfigurationRequired && selectedModelRef ? modelOptionForRef(modelOptions, selectedModelRef) : null;
  const selectedThinkingMode = normalizedThinkingSelection(state.selectedThinkingMode ?? null);
  const thinkingSupported = selectedOption?.capabilities.thinking.supported ?? false;
  const availableThinkingModes = selectedOption?.capabilities.thinking.modes ?? [];
  const nonDefaultThinkingModes = backendThinkingModeOptions.filter((mode) => availableThinkingModes.includes(mode.value));
  const thinkingMenuOptions: ThinkingModeOption[] = [defaultThinkingModeOption, ...nonDefaultThinkingModes];
  const filteredModelOptions = filterModelOptions(modelOptions, modelSearch);
  const groupedModelOptions = groupModelOptions(filteredModelOptions);
  const hasComposerContent = state.composerValue.trim().length > 0 || state.composerAttachments.length > 0;
  const imageInputSupported = modelSupportsImages(selectedOption?.capabilities, selectedModelRef);
  const imageInputSupportedRef = useRef(imageInputSupported);
  imageInputSupportedRef.current = imageInputSupported;
  const imageInputBlocked = state.composerAttachments.length > 0 && !imageInputSupported;
  const imageInputUnavailableWarning = imageInputBlocked || (imageAttachRejected && !imageInputSupported);
  const imageInputWarningVisible = imageInputUnavailableWarning || Boolean(imageAttachError);
  const imageInputWarningMessage = imageInputUnavailableWarning
    ? t("composer.attachments.imageInputUnavailable")
    : imageAttachError;
  const slashQuery = slashCommandQueryFor(state.composerValue);
  const slashCommands = buildSlashCommands({
    activeProjectId: state.activeProjectId,
    activeSessionId: state.activeSessionId,
    busy,
    planMode: state.composerPlanMode
  });
  const filteredSlashCommands =
    slashQuery !== null
      ? slashCommands.filter((command) => slashCommandMatches(command, slashQuery))
      : [];
  const slashMenuOpen =
    slashQuery !== null &&
    state.composerValue !== slashDismissedValue &&
    filteredSlashCommands.length > 0;
  const selectedSlashCommand = slashMenuOpen ? filteredSlashCommands[selectedSlashCommandIndex] : undefined;
  const activeSlashCommandId = selectedSlashCommand
    ? slashCommandItemId(slashMenuId, selectedSlashCommand.id)
    : undefined;
  const modelButtonLabel = providerConfigurationRequired
    ? t("composer.model.configureProvider")
    : selectedOption?.label ?? selectedModelRef?.model_id ?? "Choose model";

  useEffect(() => {
    if (imageInputSupported) {
      setImageAttachRejected(false);
    }
  }, [imageInputSupported, state.composerAttachments.length]);

  async function handleAddPhotos() {
    if (!imageInputSupportedRef.current) {
      setImageAttachRejected(true);
      return;
    }

    try {
      const paths = await exagentClient.pickImageFiles();
      finishImageAttach(paths);
    } catch (error) {
      setImageAttachError(errorMessage(error));
    }
  }

  async function handleAttachImagePaths(rawPaths: string[]) {
    const paths = rawPaths.filter(isSupportedImagePath);
    if (paths.length === 0) {
      return;
    }
    if (!imageInputSupportedRef.current) {
      setImageAttachRejected(true);
      return;
    }

    try {
      finishImageAttach(await exagentClient.importImagePaths(paths));
    } catch (error) {
      setImageAttachError(errorMessage(error));
    }
  }

  async function handleAttachImageFiles(files: File[]) {
    if (files.length === 0) {
      return;
    }
    if (!imageInputSupportedRef.current) {
      setImageAttachRejected(true);
      return;
    }

    try {
      finishImageAttach(await exagentClient.importImageFiles(files));
    } catch (error) {
      setImageAttachError(errorMessage(error));
    }
  }

  function finishImageAttach(paths: string[]) {
    if (paths.length > 0) {
      setImageAttachRejected(false);
      setImageAttachError(null);
    }
    addComposerAttachments(paths);
  }

  function runSlashCommand(command: SlashCommand) {
    if (command.disabled) {
      return;
    }
    setSlashDismissedValue(null);
    setComposerValue("");
    switch (command.id) {
      case "compact":
        void compactActiveThread();
        break;
      case "goal":
        openThreadGoalEditor();
        break;
      case "plan":
        setComposerPlanMode(!state.composerPlanMode);
        break;
    }
  }

  function runExactSlashCommand() {
    if (slashQuery === null) {
      return false;
    }
    const exactCommand = filteredSlashCommands.find((command) => command.command.slice(1) === slashQuery);
    if (!exactCommand || exactCommand.disabled) {
      return false;
    }
    runSlashCommand(exactCommand);
    return true;
  }

  function runSelectedSlashCommand() {
    const selected = filteredSlashCommands[selectedSlashCommandIndex];
    const command = selected && !selected.disabled
      ? selected
      : filteredSlashCommands.find((item) => !item.disabled);
    if (!command) {
      return false;
    }
    runSlashCommand(command);
    return true;
  }

  useEffect(() => {
    const owner = Symbol("composer-drag-drop-owner");
    registerDragDropComposer(owner, {
      onEnter: (paths) => {
        if (paths.length > 0) {
          setDraggingImages(paths.some(isSupportedImagePath));
        }
      },
      onLeave: () => setDraggingImages(false),
      onDrop: (paths) => {
        setDraggingImages(false);
        void handleAttachImagePaths(paths);
      }
    });

    return () => {
      unregisterDragDropComposer(owner);
    };
    // The shared native subscription is managed at module scope; callbacks read model support from a ref.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    setSelectedSlashCommandIndex(firstEnabledSlashCommandIndex(filteredSlashCommands));
  }, [slashMenuOpen, slashQuery]);

  return (
    <form
      className={cn(
        "composer-shell",
        variant === "hero" ? "rounded-xl p-3" : "rounded-lg p-2",
        draggingImages && "ring-2 ring-focus"
      )}
      aria-label="Prompt composer"
      onPaste={(event) => {
        const files = imageFilesFromFileList(event.clipboardData?.files ?? null);
        if (files.length === 0) {
          return;
        }
        event.preventDefault();
        void handleAttachImageFiles(files);
      }}
      onSubmit={(event) => {
        event.preventDefault();
        if (imageInputBlocked) {
          return;
        }
        if (busy) {
          void interruptActiveTurn();
        } else {
          void sendPrompt();
        }
      }}
    >
      <div className="relative">
        <Textarea
          className={variant === "hero" ? "min-h-[96px] border-transparent bg-transparent px-2 py-1 focus:border-transparent focus:ring-0" : undefined}
          value={state.composerValue}
          onChange={(event) => {
            setSlashDismissedValue(null);
            setComposerValue(event.target.value);
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape" && slashMenuOpen) {
              event.preventDefault();
              setSlashDismissedValue(state.composerValue);
              return;
            }
            if ((event.key === "ArrowDown" || event.key === "ArrowUp") && slashMenuOpen) {
              event.preventDefault();
              setSelectedSlashCommandIndex((current) =>
                nextEnabledSlashCommandIndex(
                  filteredSlashCommands,
                  current,
                  event.key === "ArrowDown" ? 1 : -1
                )
              );
              return;
            }
            if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) {
              return;
            }
            event.preventDefault();
            if (busy) {
              void interruptActiveTurn();
              return;
            }
            if (runExactSlashCommand()) {
              return;
            }
            if (slashMenuOpen && runSelectedSlashCommand()) {
              return;
            }
            if (imageInputBlocked) {
              return;
            }
            void sendPrompt();
          }}
          placeholder={variant === "hero" ? "Ask ExAgent to build, fix, or explain..." : "Message ExAgent"}
          aria-label="Message ExAgent"
          aria-haspopup={slashQuery !== null ? "menu" : undefined}
          aria-expanded={slashQuery !== null ? slashMenuOpen : undefined}
          aria-controls={slashMenuOpen ? slashMenuId : undefined}
          aria-activedescendant={slashMenuOpen ? activeSlashCommandId : undefined}
        />
        {slashMenuOpen ? (
          <SlashCommandMenu
            id={slashMenuId}
            activeCommandId={selectedSlashCommand?.id}
            commands={filteredSlashCommands}
            onSelect={runSlashCommand}
            onHighlight={(command) =>
              setSelectedSlashCommandIndex(
                Math.max(0, filteredSlashCommands.findIndex((item) => item.id === command.id))
              )
            }
          />
        ) : null}
      </div>
      {state.composerAttachments.length > 0 ? (
        <AttachmentTray attachments={state.composerAttachments} label={t("composer.attachments.selectedImages")} />
      ) : null}
      {imageInputWarningVisible ? (
        <div
          className="mt-2 flex items-start gap-2 rounded-md border border-warning/40 bg-warning/10 px-2.5 py-2 text-warning"
          role="alert"
        >
          <CircleAlert className="mt-0.5 h-4 w-4 shrink-0" />
          <p className="type-body-sm text-warning">{imageInputWarningMessage}</p>
        </div>
      ) : null}
      <div className="mt-2 flex items-end justify-between gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-1">
          <ComposerActionsMenu
            planMode={state.composerPlanMode}
            canUseGoal={Boolean(state.activeProjectId)}
            canAttachImages={imageInputSupported}
            onAddPhotos={() => void handleAddPhotos()}
          />

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
                <span className="truncate">{modelButtonLabel}</span>
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
                  <div className="type-body-md px-2 py-8 text-center text-subtle">
                    {providerConfigurationRequired ? t("composer.model.configureProviderDescription") : "No models found"}
                  </div>
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
            disabled={!busy && (!hasComposerContent || imageInputBlocked)}
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

function SlashCommandMenu({
  id,
  activeCommandId,
  commands,
  onSelect,
  onHighlight
}: {
  id: string;
  activeCommandId?: SlashCommandId;
  commands: SlashCommand[];
  onSelect: (command: SlashCommand) => void;
  onHighlight: (command: SlashCommand) => void;
}) {
  return (
    <div
      id={id}
      role="menu"
      aria-label="Slash commands"
      className="absolute bottom-full left-0 z-40 mb-2 w-[min(360px,calc(100vw-32px))] rounded-md border border-border bg-surface-2 p-1.5 text-ink shadow-panel"
    >
      {commands.map((command) => {
        const Icon = command.icon;
        return (
          <button
            key={command.id}
            id={slashCommandItemId(id, command.id)}
            type="button"
            role="menuitem"
            disabled={command.disabled}
            data-highlighted={command.id === activeCommandId ? "true" : undefined}
            className="flex min-h-11 w-full items-center gap-3 rounded px-2.5 py-2 text-left outline-none transition-colors hover:bg-surface-3 focus:bg-surface-3 data-[highlighted=true]:bg-surface-3 disabled:pointer-events-none disabled:opacity-45"
            onClick={() => onSelect(command)}
            onMouseEnter={() => onHighlight(command)}
          >
            <Icon className="h-4 w-4 shrink-0 text-muted" />
            <span className="min-w-0 flex-1">
              <span className="type-label-md block truncate text-ink">
                <span className="type-code-sm mr-2 text-muted">{command.command}</span>
                {command.label}
              </span>
              <span className="type-label-sm block truncate text-subtle">{command.detail}</span>
            </span>
          </button>
        );
      })}
    </div>
  );
}

function slashCommandItemId(menuId: string, commandId: SlashCommandId) {
  return `${menuId}-${commandId}`;
}

function slashCommandQueryFor(value: string) {
  if (!value.startsWith("/")) {
    return null;
  }
  const query = value.slice(1);
  if (/\s/.test(query)) {
    return null;
  }
  return query.toLowerCase();
}

function buildSlashCommands({
  activeProjectId,
  activeSessionId,
  busy,
  planMode
}: {
  activeProjectId: string | null;
  activeSessionId: string | null;
  busy: boolean;
  planMode: boolean;
}): SlashCommand[] {
  const hasActiveThread = Boolean(activeProjectId && activeSessionId);
  return [
    {
      id: "compact",
      command: "/compact",
      label: "Compact conversation",
      detail: busy ? "Available when the thread is idle" : "Summarize history and shrink context",
      icon: Database,
      disabled: !hasActiveThread || busy
    },
    {
      id: "goal",
      command: "/goal",
      label: "Set goal",
      detail: "Create or edit the thread goal",
      icon: Target,
      disabled: !activeProjectId
    },
    {
      id: "plan",
      command: "/plan",
      label: planMode ? "Disable plan mode" : "Enable plan mode",
      detail: "Toggle planning for the next prompt",
      icon: ListChecks,
      disabled: false
    }
  ];
}

function slashCommandMatches(command: SlashCommand, query: string) {
  if (!query) {
    return true;
  }
  return (
    command.command.slice(1).includes(query) ||
    command.label.toLowerCase().includes(query) ||
    command.detail.toLowerCase().includes(query)
  );
}

function firstEnabledSlashCommandIndex(commands: SlashCommand[]) {
  const index = commands.findIndex((command) => !command.disabled);
  return index >= 0 ? index : 0;
}

function nextEnabledSlashCommandIndex(commands: SlashCommand[], currentIndex: number, direction: 1 | -1) {
  if (commands.length === 0) {
    return 0;
  }
  for (let offset = 1; offset <= commands.length; offset += 1) {
    const index = (currentIndex + offset * direction + commands.length) % commands.length;
    if (!commands[index]?.disabled) {
      return index;
    }
  }
  return currentIndex;
}

const supportedImageExtensions = new Set(["png", "jpg", "jpeg", "webp", "gif"]);
const supportedImageMimeTypes = new Set(["image/png", "image/jpeg", "image/webp", "image/gif"]);

function imageFilesFromFileList(files: FileList | null): File[] {
  return Array.from(files ?? []).filter(isSupportedImageFile);
}

function isSupportedImageFile(file: File) {
  if (supportedImageMimeTypes.has(file.type.toLowerCase())) {
    return true;
  }
  return isSupportedImagePath(file.name);
}

function isSupportedImagePath(path: string) {
  const extension = path.split(".").pop()?.toLowerCase();
  return extension ? supportedImageExtensions.has(extension) : false;
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function AttachmentTray({ attachments, label }: { attachments: ComposerAttachment[]; label: string }) {
  return (
    <div className="mt-2 flex flex-wrap gap-2 px-1" aria-label={label}>
      {attachments.map((attachment) => (
        <div
          key={attachment.id}
          className="group flex h-14 w-[min(100%,17rem)] items-center gap-2 rounded-md border border-border bg-surface-1 p-1.5 text-muted transition-colors hover:border-border-strong hover:bg-surface-2"
        >
          <LocalImageThumbnail path={attachment.path} name={attachment.name} />
          <div className="min-w-0 flex-1">
            <div className="type-label-sm truncate text-ink">{attachment.name}</div>
            <div className="type-label-sm truncate text-subtle">{attachment.detail}</div>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-6 w-6 shrink-0"
            aria-label={`Remove ${attachment.name}`}
            onClick={() => removeComposerAttachment(attachment.id)}
          >
            <X className="h-3.5 w-3.5" />
          </Button>
        </div>
      ))}
    </div>
  );
}

function LocalImageThumbnail({ path, name }: { path: string; name: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return (
      <div className="flex h-11 w-14 shrink-0 items-center justify-center rounded border border-border bg-surface-2 text-subtle">
        <ImagePlus className="h-4 w-4" />
      </div>
    );
  }

  return (
    <img
      src={localFileAssetSrc(path)}
      alt={name}
      decoding="async"
      className="h-11 w-14 shrink-0 rounded border border-border bg-surface-2 object-cover"
      onError={() => setFailed(true)}
    />
  );
}

function ComposerActionsMenu({
  planMode,
  canUseGoal,
  canAttachImages,
  onAddPhotos
}: {
  planMode: boolean;
  canUseGoal: boolean;
  canAttachImages: boolean;
  onAddPhotos: () => void;
}) {
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
          onSelect={canAttachImages ? onAddPhotos : undefined}
          trailing={!canAttachImages ? <TextOnlyBadge label={t("composer.attachments.textOnly")} /> : undefined}
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

function TextOnlyBadge({ label }: { label: string }) {
  return (
    <span className="type-badge shrink-0 rounded-full border border-border bg-surface-2 px-1.5 py-1 text-subtle">
      {label}
    </span>
  );
}

function PlannedActionItem({
  icon: Icon,
  label,
  onSelect,
  trailing
}: {
  icon: LucideIcon;
  label: string;
  onSelect?: () => void;
  trailing?: ReactNode;
}) {
  return (
    <DropdownMenuItem disabled={!onSelect} className="min-h-10 gap-3 px-2.5 py-2" onSelect={onSelect}>
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
  if (isProviderConfigurationRequired(settings)) {
    return options;
  }

  if (settings?.model_options.length) {
    settings.model_options.forEach((model) => {
      addModelOptionFromView(options, seen, model, providerNameFor(settings, model.provider_id), detailForModelOption(settings, model));
    });
  } else {
    const configuredProviders = settings?.configured_providers.length
      ? settings.configured_providers
      : settings?.connected_provider
        ? [settings.config]
        : [];

    configuredProviders.forEach((provider) => {
      addFallbackModelOption(
        options,
        seen,
        provider.provider_id,
        providerNameFor(settings, provider.provider_id),
        provider.model,
        "Configured"
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

function isProviderConfigurationRequired(settings: WorkbenchState["providerSettings"]) {
  const missingRequiredCredential = Boolean(
    settings?.config.auth_required && !settings.config.has_api_key && !settings.config.has_credential
  );
  return Boolean(
    settings &&
      settings.connected_provider === null &&
      settings.configured_providers.length === 0 &&
      settings.model_options.length === 0 &&
      missingRequiredCredential
  );
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
