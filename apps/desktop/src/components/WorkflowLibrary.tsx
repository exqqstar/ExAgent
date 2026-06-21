import { useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  FileSearch,
  Layers3,
  Play,
  ShieldCheck,
  SlidersHorizontal,
  Workflow
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Textarea } from "@/components/ui/textarea";
import type { getWorkbenchState } from "@/stores/workbenchStore";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import type { ProjectSummary, SessionStatus, SessionSummary } from "@/types";

type WorkbenchState = ReturnType<typeof getWorkbenchState>;
type PresetId = "quick" | "standard" | "deep";
type TemplateId = "deep-research" | "adversarial-verify" | "fanout-synthesize";
type ConversationScope = "project" | "all";

type WorkflowTemplate = {
  id: TemplateId;
  name: string;
  zhName: string;
  summary: string;
  zhSummary: string;
  icon: typeof FileSearch;
  tags: string[];
  phaseLabel: string;
  presets: Record<PresetId, WorkflowPreset>;
};

type WorkflowPreset = {
  label: string;
  zhLabel: string;
  agents: number;
  tokens: string;
  duration: string;
  maxSources?: number;
  maxClaims?: number;
  votes?: number;
  concurrency: number;
  phases: Array<{
    name: string;
    zhName: string;
    planned: number;
  }>;
};

const WORKFLOW_RUNTIME_ENABLED = false;

const templates: WorkflowTemplate[] = [
  {
    id: "deep-research",
    name: "Deep Research",
    zhName: "深度研究",
    summary: "Search across angles, extract claims, verify them with independent voters, then produce a cited report.",
    zhSummary: "从多个角度搜索，抽取可验证论点，再用独立 verifier 投票，最后生成带来源的报告。",
    icon: FileSearch,
    tags: ["Web", "Read-only", "Heavy"],
    phaseLabel: "Scope -> Search -> Fetch -> Verify -> Synthesize",
    presets: {
      quick: {
        label: "Quick",
        zhLabel: "快速",
        agents: 29,
        tokens: "~35k",
        duration: "5-8 min",
        maxSources: 8,
        maxClaims: 8,
        votes: 2,
        concurrency: 5,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Search", zhName: "搜索", planned: 3 },
          { name: "Fetch", zhName: "抓取", planned: 8 },
          { name: "Verify", zhName: "验证", planned: 16 },
          { name: "Synthesize", zhName: "汇总", planned: 1 }
        ]
      },
      standard: {
        label: "Standard",
        zhLabel: "标准",
        agents: 42,
        tokens: "~60k",
        duration: "8-15 min",
        maxSources: 12,
        maxClaims: 12,
        votes: 2,
        concurrency: 6,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Search", zhName: "搜索", planned: 4 },
          { name: "Fetch", zhName: "抓取", planned: 12 },
          { name: "Verify", zhName: "验证", planned: 24 },
          { name: "Synthesize", zhName: "汇总", planned: 1 }
        ]
      },
      deep: {
        label: "Deep",
        zhLabel: "深入",
        agents: 82,
        tokens: "~120k",
        duration: "15-30 min",
        maxSources: 15,
        maxClaims: 20,
        votes: 3,
        concurrency: 8,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Search", zhName: "搜索", planned: 5 },
          { name: "Fetch", zhName: "抓取", planned: 15 },
          { name: "Verify", zhName: "验证", planned: 60 },
          { name: "Synthesize", zhName: "汇总", planned: 1 }
        ]
      }
    }
  },
  {
    id: "adversarial-verify",
    name: "Adversarial Verify",
    zhName: "对抗验证",
    summary: "Send a plan, report, or claim set to independent reviewers and keep only findings with evidence.",
    zhSummary: "把计划、报告或结论交给独立 reviewer 反证，只保留有证据支撑的结果。",
    icon: ShieldCheck,
    tags: ["Review", "Read-only", "Medium"],
    phaseLabel: "Load -> Review -> Vote -> Report",
    presets: {
      quick: {
        label: "Quick",
        zhLabel: "快速",
        agents: 6,
        tokens: "~12k",
        duration: "2-4 min",
        votes: 2,
        concurrency: 4,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Review", zhName: "审查", planned: 4 },
          { name: "Report", zhName: "报告", planned: 1 }
        ]
      },
      standard: {
        label: "Standard",
        zhLabel: "标准",
        agents: 11,
        tokens: "~24k",
        duration: "4-8 min",
        votes: 3,
        concurrency: 5,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Review", zhName: "审查", planned: 9 },
          { name: "Report", zhName: "报告", planned: 1 }
        ]
      },
      deep: {
        label: "Deep",
        zhLabel: "深入",
        agents: 17,
        tokens: "~40k",
        duration: "8-12 min",
        votes: 3,
        concurrency: 6,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Review", zhName: "审查", planned: 15 },
          { name: "Report", zhName: "报告", planned: 1 }
        ]
      }
    }
  },
  {
    id: "fanout-synthesize",
    name: "Fan-out Synthesize",
    zhName: "并行汇总",
    summary: "Split an open question across focused agents, then merge the strongest independent findings.",
    zhSummary: "把开放问题拆给多个聚焦 agent，再合并最有价值的独立发现。",
    icon: Layers3,
    tags: ["Explore", "Flexible", "Light"],
    phaseLabel: "Scope -> Explore -> Synthesize",
    presets: {
      quick: {
        label: "Quick",
        zhLabel: "快速",
        agents: 6,
        tokens: "~10k",
        duration: "2-5 min",
        concurrency: 4,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Explore", zhName: "探索", planned: 4 },
          { name: "Synthesize", zhName: "汇总", planned: 1 }
        ]
      },
      standard: {
        label: "Standard",
        zhLabel: "标准",
        agents: 9,
        tokens: "~18k",
        duration: "4-8 min",
        concurrency: 5,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Explore", zhName: "探索", planned: 7 },
          { name: "Synthesize", zhName: "汇总", planned: 1 }
        ]
      },
      deep: {
        label: "Deep",
        zhLabel: "深入",
        agents: 14,
        tokens: "~32k",
        duration: "8-14 min",
        concurrency: 6,
        phases: [
          { name: "Scope", zhName: "定界", planned: 1 },
          { name: "Explore", zhName: "探索", planned: 12 },
          { name: "Synthesize", zhName: "汇总", planned: 1 }
        ]
      }
    }
  }
];

export function WorkflowLibrary({ state, onOpenConversation }: { state: WorkbenchState; onOpenConversation?: () => void }) {
  const { locale } = useI18n();
  const zh = locale === "zh";
  const activeProject = state.projects.find((project) => project.id === state.activeProjectId) ?? null;
  const availableProjects = state.projects.filter((project) => !project.archived);
  const defaultProjectId = activeProject?.id ?? availableProjects[0]?.id ?? "";
  const [selectedTemplateId, setSelectedTemplateId] = useState<TemplateId | null>(null);
  const [selectedPresetId, setSelectedPresetId] = useState<PresetId>("standard");
  const [runProjectId, setRunProjectId] = useState(defaultProjectId);
  const [conversationScope, setConversationScope] = useState<ConversationScope>("project");
  const [question, setQuestion] = useState(
    "研究一下 world model 相关的发展历程、技术路线以及相关应用，覆盖 World Models、Dreamer、JEPA、Genie 和视频生成模型。"
  );
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const selectedTemplate = selectedTemplateId ? templates.find((template) => template.id === selectedTemplateId) ?? null : null;
  const selectedPreset = selectedTemplate?.presets[selectedPresetId] ?? templates[0].presets.standard;
  const resolvedRunProjectId =
    runProjectId && availableProjects.some((project) => project.id === runProjectId) ? runProjectId : defaultProjectId;
  const selectedRunProject = availableProjects.find((project) => project.id === resolvedRunProjectId) ?? null;
  const relatedConversations = relatedWorkflowConversations(state.sessions, resolvedRunProjectId, conversationScope);
  const canStartWorkflow =
    WORKFLOW_RUNTIME_ENABLED &&
    selectedTemplate?.id === "deep-research" &&
    Boolean(selectedRunProject) &&
    Boolean(question.trim()) &&
    !starting;

  function selectTemplate(templateId: TemplateId) {
    setSelectedTemplateId(templateId);
    setSelectedPresetId("standard");
    setRunProjectId(defaultProjectId);
    setConversationScope("project");
    setStartError(null);
  }

  async function startWorkflow() {
    const trimmedQuestion = question.trim();
    if (selectedTemplate?.id !== "deep-research" || !selectedRunProject || !trimmedQuestion) {
      return;
    }

    setStarting(true);
    setStartError(null);
    try {
      const threadId = await state.startWorkflow(selectedRunProject.id, {
        templateId: "deep-research",
        presetId: selectedPresetId,
        question: trimmedQuestion
      });
      if (threadId) {
        onOpenConversation?.();
      } else {
        setStartError(zh ? "启动 workflow 失败。" : "Workflow start failed.");
      }
    } finally {
      setStarting(false);
    }
  }

  function openConversation(session: SessionSummary) {
    onOpenConversation?.();
    if (session.projectId !== state.activeProjectId) {
      void state.selectProject(session.projectId, session.id);
      return;
    }
    void state.openSession(session.id);
  }

  if (!selectedTemplate) {
    return (
      <WorkflowTemplateGallery
        activeProjectName={activeProject?.name ?? null}
        templates={templates}
        zh={zh}
        onSelectTemplate={selectTemplate}
      />
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-b border-border px-4 py-3">
        <div className="mx-auto flex max-w-[1280px] items-center justify-between gap-3">
          <div className="flex min-w-0 items-center gap-2">
            <Button type="button" variant="ghost" size="sm" onClick={() => setSelectedTemplateId(null)}>
              <ArrowLeft className="h-4 w-4" />
              {zh ? "模板库" : "Templates"}
            </Button>
            <div className="min-w-0 border-l border-border pl-3">
              <div className="flex min-w-0 items-center gap-2">
                <h1 className="type-title-lg truncate text-ink">{zh ? selectedTemplate.zhName : selectedTemplate.name}</h1>
                <Badge variant="warning" className="shrink-0">
                  {zh ? "实施中" : "In progress"}
                </Badge>
              </div>
              <p className="type-body-sm mt-0.5 truncate text-muted">
                {zh ? "配置一次可继续的 workflow 对话。" : "Configure a resumable workflow conversation."}
              </p>
            </div>
          </div>
        </div>
      </div>

      <div className="min-h-0 flex-1">
        <ScrollArea className="h-full">
          <div className="mx-auto flex w-full max-w-[820px] flex-col gap-4 px-4 py-6">
            <section className="rounded-lg border border-warning/30 bg-warning/10 px-3 py-2" aria-label={zh ? "工作流状态" : "Workflow status"}>
              <p className="type-body-sm text-ink">
                {zh
                  ? "Workflow 功能实施中。当前页面保留为路线图预览和历史对话入口，新 workflow 启动暂时关闭。"
                  : "Workflows are in progress. This page remains as a roadmap preview and history entry point; starting new workflow runs is temporarily disabled."}
              </p>
            </section>

            <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
              <p className="type-body-md max-w-[58ch] text-muted">{zh ? selectedTemplate.zhSummary : selectedTemplate.summary}</p>
              <ProjectPicker
                projects={availableProjects}
                selectedProjectId={resolvedRunProjectId}
                zh={zh}
                onChange={(projectId) => {
                  setRunProjectId(projectId);
                  setConversationScope("project");
                  setStartError(null);
                }}
              />
            </div>

            <section aria-label={zh ? "问题" : "Question"} className="space-y-2">
              <h2 className="type-title-sm text-ink">
                {selectedTemplate.id === "deep-research"
                  ? zh
                    ? "研究问题"
                    : "Research question"
                  : zh
                    ? "任务目标"
                    : "Task target"}
              </h2>
              <div className="overflow-hidden rounded-xl border border-border bg-surface-1">
                <Textarea
                  value={question}
                  onChange={(event) => {
                    setQuestion(event.target.value);
                    setStartError(null);
                  }}
                  className="min-h-[190px] rounded-none border-0 bg-transparent px-4 py-3 shadow-none"
                  placeholder={zh ? "描述你想让这个 workflow 处理的问题..." : "Describe what this workflow should handle..."}
                />
                <div className="flex flex-col gap-3 border-t border-border bg-surface-2/45 p-3 sm:flex-row sm:items-end">
                  <div className="min-w-0 flex-1 space-y-1.5">
                    <p className="type-label-sm text-muted">{zh ? "运行强度" : "Run intensity"}</p>
                    <div className="grid max-w-[360px] grid-cols-3 gap-1 rounded-lg border border-border bg-surface-1 p-1">
                      {(["quick", "standard", "deep"] as const).map((presetId) => {
                        const preset = selectedTemplate.presets[presetId];
                        const active = selectedPresetId === presetId;
                        return (
                          <button
                            key={presetId}
                            type="button"
                            className={cn(
                              "type-label-sm rounded-md px-2 py-1.5 text-muted transition-colors duration-150 hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
                              active && "bg-surface-2 text-ink shadow-[inset_0_0_0_1px_color-mix(in_oklch,var(--color-border)_86%,transparent)]"
                            )}
                            onClick={() => {
                              setSelectedPresetId(presetId);
                              setStartError(null);
                            }}
                          >
                            {zh ? preset.zhLabel : preset.label}
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  <Button type="button" disabled={!canStartWorkflow} onClick={() => void startWorkflow()}>
                    <Play className="h-4 w-4" />
                    {starting
                      ? zh
                        ? "启动中"
                        : "Starting"
                      : !WORKFLOW_RUNTIME_ENABLED
                        ? zh
                          ? "实施中"
                          : "In progress"
                      : selectedTemplate.id === "deep-research"
                        ? zh
                          ? "启动 workflow"
                          : "Start workflow"
                        : zh
                          ? "即将支持"
                          : "Coming soon"}
                  </Button>
                </div>
                {!WORKFLOW_RUNTIME_ENABLED ? (
                  <p className="type-body-xs border-t border-border px-4 py-2 text-muted">
                    {zh
                      ? "自动找源、抓取和验证流程还在收敛，开源版本先不暴露运行入口。"
                      : "Automatic source discovery, fetching, and verification are still being shaped, so the public build does not expose the run entry yet."}
                  </p>
                ) : selectedTemplate.id !== "deep-research" ? (
                  <p className="type-body-xs border-t border-border px-4 py-2 text-muted">
                    {zh ? "这个模板还没有接入 runtime。" : "This template is not wired to the runtime yet."}
                  </p>
                ) : startError ? (
                  <p className="type-body-xs border-t border-border px-4 py-2 text-danger" role="alert">
                    {startError}
                  </p>
                ) : null}
              </div>
            </section>

            <RelatedConversations
              conversations={relatedConversations}
              projects={availableProjects}
              scope={conversationScope}
              selectedProject={selectedRunProject}
              zh={zh}
              onOpenConversation={openConversation}
              onScopeChange={setConversationScope}
            />

            <details className="rounded-lg border border-border bg-surface-1 px-3 py-2">
              <summary className="type-label-md flex cursor-default list-none items-center gap-2 text-muted">
                <SlidersHorizontal className="h-4 w-4 text-muted" />
                {zh ? "高级参数" : "Advanced parameters"}
              </summary>
              <div className="mt-3 grid gap-2 sm:grid-cols-2">
                <ParameterRow label={zh ? "预算" : "Budget"} value={selectedPreset.tokens} />
                <ParameterRow label={zh ? "来源上限" : "Max sources"} value={selectedPreset.maxSources ?? "N/A"} />
                <ParameterRow label={zh ? "论点上限" : "Max claims"} value={selectedPreset.maxClaims ?? "N/A"} />
                <ParameterRow label={zh ? "每条投票" : "Votes per item"} value={selectedPreset.votes ?? "N/A"} />
                <ParameterRow label={zh ? "并发" : "Concurrency"} value={selectedPreset.concurrency} />
              </div>
            </details>
          </div>
        </ScrollArea>
      </div>
    </div>
  );
}

function WorkflowTemplateGallery({
  activeProjectName,
  templates,
  zh,
  onSelectTemplate
}: {
  activeProjectName: string | null;
  templates: WorkflowTemplate[];
  zh: boolean;
  onSelectTemplate: (id: TemplateId) => void;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-b border-border px-4 py-3">
        <div className="mx-auto flex max-w-[1280px] items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="flex min-w-0 items-center gap-2">
              <h1 className="type-title-lg text-ink">{zh ? "工作流模板" : "Workflow templates"}</h1>
              <Badge variant="warning" className="shrink-0">
                {zh ? "实施中" : "In progress"}
              </Badge>
            </div>
            <p className="type-body-sm mt-0.5 truncate text-muted">
              {activeProjectName
                ? zh
                  ? `当前项目: ${activeProjectName}`
                  : `Current project: ${activeProjectName}`
                : zh
                  ? "先选择一个预设 workflow，再进入详情配置目标和运行规模。"
                  : "Choose a workflow first, then configure target and run scale."}
              </p>
            </div>
        </div>
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto flex w-full max-w-[1080px] flex-col gap-4 px-3 py-4">
          <section className="flex flex-col gap-2 sm:flex-row sm:items-end sm:justify-between">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <Workflow className="h-4 w-4 text-muted" />
                <h2 className="type-title-md text-ink">{zh ? "选择 workflow" : "Select a workflow"}</h2>
              </div>
            </div>
          </section>

          <section
            aria-label={zh ? "可用 workflow" : "Available workflows"}
            className="grid grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-3"
          >
            {templates.map((template) => (
              <WorkflowSelectionCard key={template.id} template={template} zh={zh} onSelectTemplate={onSelectTemplate} />
            ))}
          </section>
        </div>
      </ScrollArea>
    </div>
  );
}

function WorkflowSelectionCard({
  template,
  zh,
  onSelectTemplate
}: {
  template: WorkflowTemplate;
  zh: boolean;
  onSelectTemplate: (id: TemplateId) => void;
}) {
  const Icon = template.icon;
  return (
    <button
      type="button"
      className="group flex min-h-[122px] w-full flex-col rounded-xl border border-border bg-surface-1 p-4 text-left transition-[background-color,border-color,transform] duration-150 hover:-translate-y-0.5 hover:border-border-strong hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
      onClick={() => onSelectTemplate(template.id)}
    >
      <span className="flex items-center justify-between gap-3">
        <span className="flex min-w-0 items-center gap-2">
          <span className="rounded-lg border border-border bg-surface-2 p-2 text-ink transition-colors duration-150 group-hover:bg-surface-3">
            <Icon className="h-4 w-4" />
          </span>
          <span className="type-title-md truncate text-ink">{zh ? template.zhName : template.name}</span>
          <Badge variant="warning" className="shrink-0">
            {zh ? "实施中" : "In progress"}
          </Badge>
        </span>
        <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-border text-muted transition-colors duration-150 group-hover:border-border-strong group-hover:text-ink">
          <ArrowRight className="h-4 w-4" />
        </span>
      </span>

      <span className="mt-3 min-w-0">
        <span className="type-body-sm line-clamp-2 text-muted">{zh ? template.zhSummary : template.summary}</span>
      </span>
    </button>
  );
}

function ProjectPicker({
  projects,
  selectedProjectId,
  zh,
  onChange
}: {
  projects: ProjectSummary[];
  selectedProjectId: string;
  zh: boolean;
  onChange: (projectId: string) => void;
}) {
  if (projects.length === 0) {
    return (
      <div className="shrink-0 rounded-lg border border-border bg-surface-1 px-3 py-2">
        <p className="type-label-sm text-muted">{zh ? "运行项目" : "Run in"}</p>
        <p className="type-body-sm mt-0.5 text-ink">{zh ? "未选择项目" : "No project selected"}</p>
      </div>
    );
  }

  return (
    <label className="shrink-0 space-y-1.5">
      <span className="type-label-sm block text-muted">{zh ? "运行项目" : "Run in"}</span>
      <select
        className="type-body-sm h-9 min-w-[180px] rounded-lg border border-border bg-surface-1 px-3 text-ink shadow-sm outline-none transition-colors hover:bg-surface-2 focus:border-focus focus:ring-2 focus:ring-focus/20"
        value={selectedProjectId}
        onChange={(event) => onChange(event.target.value)}
      >
        {projects.map((project) => (
          <option key={project.id} value={project.id}>
            {project.name}
          </option>
        ))}
      </select>
    </label>
  );
}

function RelatedConversations({
  conversations,
  projects,
  scope,
  selectedProject,
  zh,
  onOpenConversation,
  onScopeChange
}: {
  conversations: SessionSummary[];
  projects: ProjectSummary[];
  scope: ConversationScope;
  selectedProject: ProjectSummary | null;
  zh: boolean;
  onOpenConversation: (session: SessionSummary) => void;
  onScopeChange: (scope: ConversationScope) => void;
}) {
  return (
    <section className="space-y-2" aria-label={zh ? "相关对话" : "Related conversations"}>
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="min-w-0">
          <h2 className="type-title-sm text-ink">{zh ? "相关对话" : "Related conversations"}</h2>
          <p className="type-body-sm mt-0.5 truncate text-muted">
            {selectedProject
              ? zh
                ? `默认归入 ${selectedProject.name}`
                : `Defaults to ${selectedProject.name}`
              : zh
                ? "新 run 会成为一条可继续的对话"
                : "New runs become resumable conversations"}
          </p>
        </div>

        <div className="grid w-full max-w-[260px] grid-cols-2 gap-1 rounded-lg border border-border bg-surface-1 p-1 sm:w-auto">
          {(["project", "all"] as const).map((nextScope) => {
            const active = scope === nextScope;
            return (
              <button
                key={nextScope}
                type="button"
                className={cn(
                  "type-label-sm rounded-md px-2.5 py-1.5 text-muted transition-colors duration-150 hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
                  active && "bg-surface-2 text-ink shadow-[inset_0_0_0_1px_color-mix(in_oklch,var(--color-border)_86%,transparent)]"
                )}
                onClick={() => onScopeChange(nextScope)}
              >
                {nextScope === "project" ? (zh ? "当前项目" : "Project") : zh ? "全部" : "All"}
              </button>
            );
          })}
        </div>
      </div>

      <div className="overflow-hidden rounded-xl border border-border bg-surface-1">
        {conversations.length > 0 ? (
          conversations.map((conversation, index) => {
            const projectName = projects.find((project) => project.id === conversation.projectId)?.name;
            return (
              <button
                key={conversation.id}
                type="button"
                className={cn(
                  "flex w-full items-center gap-3 px-3 py-2.5 text-left transition-colors hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus",
                  index > 0 && "border-t border-border"
                )}
                onClick={() => onOpenConversation(conversation)}
              >
                <span className={cn("h-2 w-2 shrink-0 rounded-full", sessionStatusClass(conversation.status))} />
                <span className="min-w-0 flex-1">
                  <span className="type-body-sm block truncate text-ink">{conversation.title || (zh ? "未命名对话" : "Untitled conversation")}</span>
                  {scope === "all" && projectName ? (
                    <span className="type-label-sm mt-0.5 block truncate text-muted">{projectName}</span>
                  ) : null}
                </span>
                <ArrowRight className="h-4 w-4 shrink-0 text-muted" />
              </button>
            );
          })
        ) : (
          <div className="px-3 py-4">
            <p className="type-body-sm text-muted">
              {zh ? "这个范围里还没有 workflow 对话。新的运行会出现在这里。" : "No workflow conversations in this scope yet. New runs will appear here."}
            </p>
          </div>
        )}
      </div>
    </section>
  );
}

function ParameterRow({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="flex items-center justify-between gap-3 rounded-md bg-surface-1/70 px-2.5 py-2">
      <span className="type-body-sm text-muted">{label}</span>
      <span className="type-code-sm text-ink">{value}</span>
    </div>
  );
}

function relatedWorkflowConversations(sessions: SessionSummary[], projectId: string, scope: ConversationScope) {
  return [...sessions]
    .filter((session) => !session.archived)
    .filter((session) => scope === "all" || !projectId || session.projectId === projectId)
    .sort((a, b) => sessionSortValue(b) - sessionSortValue(a))
    .slice(0, 5);
}

function sessionSortValue(session: SessionSummary) {
  const updatedAt = Date.parse(session.updatedAt);
  if (Number.isFinite(updatedAt)) {
    return updatedAt;
  }
  return session.createdAt ?? 0;
}

function sessionStatusClass(status: SessionStatus) {
  switch (status) {
    case "running":
      return "bg-success motion-safe:animate-pulse";
    case "awaiting_approval":
      return "bg-warning";
    case "failed":
      return "bg-danger";
    case "archived":
      return "bg-muted";
    case "idle":
    default:
      return "bg-muted";
  }
}
