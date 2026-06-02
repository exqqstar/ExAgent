import { ArrowUp, Brain, Paperclip, Square } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  applyRuntimePreset,
  interruptActiveTurn,
  sendPrompt,
  setComposerValue,
  setSelectedModel,
  setSelectedThinkingMode
} from "@/stores/workbenchStore";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import type { ThinkingMode } from "@/types";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;

const thinkingModes: Array<{ value: ThinkingMode; label: string }> = [
  { value: "auto", label: "Auto" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" }
];

export function Composer({ state }: { state: WorkbenchState }) {
  const running = state.sessions.find((session) => session.id === state.activeSessionId)?.status === "running";
  const selectedThinkingMode = state.selectedThinkingMode ?? state.runtimeSettings?.default_thinking_mode ?? "auto";
  const selectedModel = state.selectedModel?.model_id ?? state.runtimeSettings?.default_model ?? "";

  return (
    <form
      className="rounded-lg border border-border bg-surface-1 p-2"
      aria-label="Prompt composer"
      onSubmit={(event) => {
        event.preventDefault();
        void sendPrompt();
      }}
    >
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <div className="flex h-8 min-w-[180px] flex-1 items-center gap-2 rounded-md border border-border bg-surface-2 px-2 text-xs text-muted">
          <span>Model</span>
          <input
            aria-label="Composer model"
            className="min-w-0 flex-1 bg-transparent font-mono text-xs text-ink outline-none"
            value={selectedModel}
            onChange={(event) => setSelectedModel(event.target.value)}
          />
        </div>

        <div className="flex items-center gap-1" aria-label="Thinking mode">
          <Brain className="h-4 w-4 text-subtle" />
          {thinkingModes.map((mode) => (
            <Button
              key={mode.value}
              type="button"
              variant={selectedThinkingMode === mode.value ? "secondary" : "ghost"}
              size="sm"
              aria-label={`Thinking ${mode.value}`}
              aria-pressed={selectedThinkingMode === mode.value}
              onClick={() => setSelectedThinkingMode(mode.value)}
            >
              {mode.label}
            </Button>
          ))}
        </div>

        {state.runtimeSettings?.presets.length ? (
          <select
            aria-label="Runtime preset"
            className="h-8 rounded-md border border-border bg-surface-2 px-2 text-xs text-ink outline-none focus:ring-2 focus:ring-focus"
            defaultValue=""
            onChange={(event) => {
              if (event.target.value) {
                applyRuntimePreset(event.target.value);
              }
            }}
          >
            <option value="">Preset</option>
            {state.runtimeSettings.presets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {preset.name}
              </option>
            ))}
          </select>
        ) : null}
      </div>

      <Textarea
        value={state.composerValue}
        onChange={(event) => setComposerValue(event.target.value)}
        placeholder="Message ExAgent"
        aria-label="Message ExAgent"
      />
      <div className="mt-2 flex items-center justify-between gap-2">
        <div className="flex items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button type="button" variant="ghost" size="icon" aria-label="Attach context">
                <Paperclip className="h-4 w-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Attach context</TooltipContent>
          </Tooltip>
        </div>
        <div className="flex items-center gap-2">
          {running ? (
            <Button type="button" variant="secondary" onClick={() => void interruptActiveTurn()}>
              <Square className="h-3.5 w-3.5" />
              Interrupt
            </Button>
          ) : null}
          <Button type="submit" disabled={!state.composerValue.trim()} aria-label="Send message">
            <ArrowUp className="h-4 w-4" />
            Send
          </Button>
        </div>
      </div>
    </form>
  );
}
