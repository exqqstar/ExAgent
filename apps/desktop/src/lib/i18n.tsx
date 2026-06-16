import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

export type Locale = "en" | "zh";

export type TranslationKey =
  | "approvals.inbox.title"
  | "approvals.inbox.description"
  | "approvals.inbox.empty"
  | "approvals.inbox.pending"
  | "approvals.inbox.approvalSingular"
  | "approvals.inbox.approvalPlural"
  | "approvals.inbox.loading"
  | "approvals.inbox.approveSelected"
  | "approvals.inbox.approveSelectedAria"
  | "approvals.inbox.clearSelection"
  | "approvals.inbox.details"
  | "approvals.inbox.detailsFor"
  | "approvals.inbox.hideDetails"
  | "approvals.inbox.hideDetailsFor"
  | "approvals.inbox.approve"
  | "approvals.inbox.approveFor"
  | "approvals.inbox.reject"
  | "approvals.inbox.rejectFor"
  | "approvals.inbox.rejectRollback"
  | "approvals.inbox.rejectRollbackFor"
  | "approvals.inbox.resolveQuestion"
  | "approvals.inbox.resolveQuestionFor"
  | "approvals.inbox.answerFor"
  | "approvals.inbox.answerPlaceholder"
  | "approvals.inbox.rollbackUnavailable"
  | "approvals.inbox.checkpoint"
  | "approvals.inbox.confirmTitle"
  | "approvals.inbox.confirmDescription"
  | "approvals.inbox.confirmUnderstanding"
  | "approvals.inbox.confirmAction"
  | "approvals.inbox.cancel"
  | "approvals.inbox.kind"
  | "approvals.inbox.requested"
  | "approvals.inbox.requestedUnknown"
  | "approvals.inbox.select"
  | "approvals.inbox.groupGoal"
  | "approvals.inbox.groupThread"
  | "approvals.inbox.status.approved"
  | "approvals.inbox.status.denied"
  | "approvals.inbox.status.openQuestionResolved"
  | "approvals.inbox.status.batchApproved"
  | "approvals.inbox.status.batchPartialFailed"
  | "approvals.inbox.status.rollbackUnavailable"
  | "approvals.inbox.status.rollbackRestored"
  | "approvals.inbox.status.rollbackFailedAfterReject"
  | "chrome.sidebar.hideProjectSidebar"
  | "chrome.sidebar.showProjectSidebar"
  | "chrome.sidebar.hideSidebar"
  | "chrome.sidebar.showSidebar"
  | "chrome.sidebar.open"
  | "chrome.sidebar.projectsAndSessions"
  | "chrome.sidebar.resizeProjectSidebar"
  | "chrome.navigation.back"
  | "chrome.navigation.forward"
  | "chrome.memory.open"
  | "chrome.memory.title"
  | "chrome.memory.description"
  | "chrome.inspector.open"
  | "chrome.inspector.title"
  | "chrome.session.new"
  | "sidebar.project"
  | "sidebar.newChat"
  | "sidebar.search"
  | "sidebar.projects"
  | "sidebar.addProject"
  | "sidebar.openSettings"
  | "sidebar.settings"
  | "sidebar.newSession"
  | "sidebar.newSessionFor"
  | "sidebar.projectActionsFor"
  | "sidebar.sessionActionsFor"
  | "sidebar.forkedSessionLabel"
  | "sidebar.archiveSession"
  | "sidebar.archiveSessionFor"
  | "sidebar.renameSession"
  | "sidebar.pinSession"
  | "sidebar.unpinSession"
  | "sidebar.compareWithParent"
  | "sidebar.pinProject"
  | "sidebar.unpinProject"
  | "sidebar.showInFinder"
  | "sidebar.createPermanentWorktree"
  | "sidebar.renameProject"
  | "sidebar.markAllRead"
  | "sidebar.archiveConversations"
  | "sidebar.archiveProject"
  | "sidebar.removeFromSidebar"
  | "sidebar.loadingSessions"
  | "sidebar.noSessions"
  | "sidebar.searchDialog.title"
  | "sidebar.searchDialog.description"
  | "sidebar.searchDialog.placeholder"
  | "sidebar.searchDialog.noProject"
  | "sidebar.searchDialog.noMatches"
  | "sidebar.renameSession.title"
  | "sidebar.renameSession.description"
  | "sidebar.renameSession.field"
  | "sidebar.renameProject.title"
  | "sidebar.renameProject.description"
  | "sidebar.renameProject.field"
  | "sidebar.confirm.archiveProject.title"
  | "sidebar.confirm.archiveProject.description"
  | "sidebar.confirm.archiveProject.action"
  | "sidebar.confirm.archiveConversations.title"
  | "sidebar.confirm.archiveConversations.description"
  | "sidebar.confirm.archiveConversations.action"
  | "sidebar.confirm.removeProject.title"
  | "sidebar.confirm.removeProject.description"
  | "sidebar.confirm.removeProject.action"
  | "chat.inspector"
  | "chat.compare.sharedTurnSingular"
  | "chat.compare.sharedTurnPlural"
  | "chat.compare.close"
  | "chat.compare.parentLabel"
  | "chat.compare.parentEyebrow"
  | "chat.compare.forkLabel"
  | "chat.compare.forkEyebrow"
  | "chat.compare.empty"
  | "chat.empty.title"
  | "chat.empty.addProject.title"
  | "chat.empty.addProject.description"
  | "chat.empty.addProject.action"
  | "chat.prompt.buildFeature.title"
  | "chat.prompt.buildFeature.description"
  | "chat.prompt.buildFeature.value"
  | "chat.prompt.fixProblem.title"
  | "chat.prompt.fixProblem.description"
  | "chat.prompt.fixProblem.value"
  | "chat.prompt.reviewCode.title"
  | "chat.prompt.reviewCode.description"
  | "chat.prompt.reviewCode.value"
  | "status.session.idle"
  | "status.session.running"
  | "status.session.awaitingApproval"
  | "status.session.failed"
  | "status.session.archived"
  | "status.agent.running"
  | "status.agent.spawning"
  | "status.agent.waitingApproval"
  | "status.agent.done"
  | "status.agent.idle"
  | "status.agent.failed"
  | "inspector.sections.progress"
  | "inspector.sections.agents"
  | "inspector.sections.environment"
  | "inspector.sections.runtime"
  | "inspector.sections.tokenUsage"
  | "inspector.sections.contextWindow"
  | "inspector.sections.changedFiles"
  | "inspector.sections.events"
  | "inspector.agentSummary.running"
  | "inspector.agentSummary.singular"
  | "inspector.agentSummary.plural"
  | "inspector.enabledCount"
  | "inspector.context.percentUsed"
  | "inspector.context.notReported"
  | "inspector.context.empty"
  | "inspector.changedFiles.none"
  | "inspector.changedFiles.changed"
  | "inspector.changedFiles.empty"
  | "inspector.events.recorded"
  | "inspector.events.empty"
  | "inspector.waitingApproval.approvalSingular"
  | "inspector.waitingApproval.approvalPlural"
  | "inspector.waitingApproval.expandSingular"
  | "inspector.waitingApproval.expandPlural"
  | "agents.treeLabel"
  | "agents.expand"
  | "agents.collapse"
  | "agents.openThread"
  | "agents.inspect"
  | "agents.activity"
  | "agents.tool"
  | "agents.tokens"
  | "tokenUsage.empty"
  | "tokenUsage.threadTotal"
  | "tokenUsage.input"
  | "tokenUsage.output"
  | "tokenUsage.reasoning"
  | "tokenUsage.cachedInput"
  | "tokenUsage.lastTurn"
  | "tokenUsage.lastInput"
  | "tokenUsage.lastOutput"
  | "tokenUsage.summary.notReported"
  | "tokenUsage.summary.tokens"
  | "goal.action"
  | "goal.objective"
  | "goal.tokenBudget"
  | "goal.cancelEdit"
  | "goal.save"
  | "goal.pause"
  | "goal.resume"
  | "goal.edit"
  | "goal.clear"
  | "goal.mode"
  | "goal.modeFor"
  | "goal.mode.standard"
  | "goal.mode.reviewed"
  | "goal.mode.intensive"
  | "goal.mode.standardTitle"
  | "goal.mode.reviewedTitle"
  | "goal.mode.intensiveTitle"
  | "goal.status.draft"
  | "goal.status.active"
  | "goal.status.complete"
  | "goal.status.blocked"
  | "goal.status.budgetLimited"
  | "goal.status.usageLimited"
  | "goal.status.paused"
  | "goal.usage.tokens"
  | "goal.usage.left"
  | "transcript.actions.copyReply"
  | "transcript.actions.copiedReply"
  | "transcript.actions.forkFromHere"
  | "transcript.actions.forkFromReply"
  | "sessions.forkedFromTurn"
  | "composer.aria.promptComposer"
  | "composer.aria.message"
  | "composer.aria.openActions"
  | "composer.placeholder.hero"
  | "composer.placeholder.dock"
  | "composer.plan.enabled"
  | "composer.plan.short"
  | "composer.runtimePreset"
  | "composer.runtimePreset.default"
  | "composer.model.choose"
  | "composer.model.button"
  | "composer.model.search"
  | "composer.model.noModels"
  | "composer.model.configured"
  | "composer.model.selected"
  | "composer.model.providerDefault"
  | "composer.model.available"
  | "composer.thinking.mode"
  | "composer.thinking.default"
  | "composer.thinking.defaultAria"
  | "composer.thinking.off"
  | "composer.thinking.offAria"
  | "composer.thinking.minimal"
  | "composer.thinking.minimalAria"
  | "composer.thinking.low"
  | "composer.thinking.lowAria"
  | "composer.thinking.medium"
  | "composer.thinking.mediumAria"
  | "composer.thinking.high"
  | "composer.thinking.highAria"
  | "composer.thinking.xHigh"
  | "composer.thinking.xHighAria"
  | "composer.slash.label"
  | "composer.slash.compact.label"
  | "composer.slash.compact.detail"
  | "composer.slash.compact.busyDetail"
  | "composer.slash.goal.label"
  | "composer.slash.goal.detail"
  | "composer.slash.plan.enable"
  | "composer.slash.plan.disable"
  | "composer.slash.plan.detail"
  | "composer.send"
  | "composer.interrupt"
  | "composer.actions.addPhotosAndFiles"
  | "composer.actions.attachChrome"
  | "composer.actions.planMode"
  | "composer.actions.goal"
  | "composer.actions.plugins"
  | "composer.attachments.imageInputUnavailable"
  | "composer.attachments.selectedImages"
  | "composer.attachments.removeFor"
  | "composer.attachments.textOnly"
  | "composer.model.configureProvider"
  | "composer.model.configureProviderDescription"
  | "settings.title"
  | "settings.description"
  | "settings.sections.general"
  | "settings.sections.providers"
  | "settings.sections.mcp"
  | "settings.sections.skills"
  | "settings.sections.webSearch"
  | "settings.sections.archive"
  | "settings.sections.aria"
  | "settings.general.title"
  | "settings.general.description"
  | "settings.theme.title"
  | "settings.theme.description"
  | "settings.theme.current"
  | "settings.theme.system.title"
  | "settings.theme.system.description"
  | "settings.theme.light.title"
  | "settings.theme.light.description"
  | "settings.theme.dark.title"
  | "settings.theme.dark.description"
  | "settings.theme.note"
  | "settings.language.title"
  | "settings.language.description"
  | "settings.language.current"
  | "settings.language.english.title"
  | "settings.language.english.description"
  | "settings.language.chinese.title"
  | "settings.language.chinese.description"
  | "settings.language.note"
  | "settings.providers.title"
  | "settings.providers.description"
  | "settings.providers.popular"
  | "settings.providers.loading"
  | "settings.providers.back"
  | "settings.providers.connect"
  | "settings.providers.configure"
  | "settings.providers.recommended"
  | "settings.providers.active"
  | "settings.providers.planned"
  | "settings.providers.descriptions.openai"
  | "settings.providers.descriptions.openaiCompatible"
  | "settings.providers.descriptions.anthropic"
  | "settings.providers.descriptions.githubCopilot"
  | "settings.providers.descriptions.google"
  | "settings.providers.descriptions.deepseek"
  | "settings.providers.descriptions.kimi"
  | "settings.providers.descriptions.glm"
  | "settings.connection.chooseOpenAi"
  | "settings.connection.openAiAuthMode"
  | "settings.connection.openAiApiKey"
  | "settings.connection.apiKey"
  | "settings.connection.savedInKeychain"
  | "settings.connection.compatibleDescription"
  | "settings.connection.apiKeyDescription"
  | "settings.connection.baseUrl"
  | "settings.connection.baseUrlPresets"
  | "settings.connection.baseUrlPresetsAria"
  | "settings.connection.baseUrlPresetInternational"
  | "settings.connection.baseUrlPresetMainlandChina"
  | "settings.connection.model"
  | "settings.connection.discovering"
  | "settings.connection.discoverModels"
  | "settings.connection.noModels"
  | "settings.connection.clearSavedApiKey"
  | "settings.connection.saveProvider"
  | "settings.connection.saving"
  | "settings.connection.testConnection"
  | "settings.connection.testing"
  | "settings.connection.startOAuth"
  | "settings.connection.startingOAuth"
  | "settings.connection.completeOAuth"
  | "settings.connection.completingOAuth"
  | "settings.connection.oauthCode"
  | "settings.connection.openOAuthPage"
  | "settings.connection.plannedChatGpt"
  | "settings.connection.plannedProvider"
  | "settings.connection.comingSoon"
  | "settings.connection.githubDeployment"
  | "settings.connection.githubPublic"
  | "settings.connection.githubEnterpriseDescription"
  | "settings.mcp.title"
  | "settings.mcp.description"
  | "settings.mcp.add"
  | "settings.mcp.empty"
  | "settings.mcp.save"
  | "settings.mcp.newName"
  | "settings.skills.title"
  | "settings.skills.description"
  | "settings.skills.add"
  | "settings.skills.empty"
  | "settings.skills.save"
  | "settings.skills.newName"
  | "settings.archive.title"
  | "settings.archive.description"
  | "settings.archive.refresh"
  | "settings.archive.loading"
  | "settings.archive.emptyTitle"
  | "settings.archive.emptyDescription"
  | "settings.archive.archived"
  | "settings.archive.restore"
  | "settings.archive.open"
  | "common.enabled"
  | "common.remove"
  | "common.name"
  | "common.command"
  | "common.arguments"
  | "common.workingDirectory"
  | "common.environment"
  | "common.scope"
  | "common.path"
  | "common.saving"
  | "common.current"
  | "common.cancel"
  | "common.save"
  | "common.close";

type I18nContextValue = {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: (key: TranslationKey) => string;
};

const localeStorageKey = "exagent.locale";

const translations: Record<Locale, Record<TranslationKey, string>> = {
  en: {
    "approvals.inbox.title": "Approval inbox",
    "approvals.inbox.description": "Review pending approvals across active threads.",
    "approvals.inbox.empty": "No pending approvals.",
    "approvals.inbox.pending": "pending",
    "approvals.inbox.approvalSingular": "approval",
    "approvals.inbox.approvalPlural": "approvals",
    "approvals.inbox.loading": "Refreshing approvals...",
    "approvals.inbox.approveSelected": "Approve selected",
    "approvals.inbox.approveSelectedAria": "Approve selected approvals",
    "approvals.inbox.clearSelection": "Clear selection",
    "approvals.inbox.details": "Show details",
    "approvals.inbox.detailsFor": "Show details for {summary}",
    "approvals.inbox.hideDetails": "Hide details",
    "approvals.inbox.hideDetailsFor": "Hide details for {summary}",
    "approvals.inbox.approve": "Approve",
    "approvals.inbox.approveFor": "Approve {summary}",
    "approvals.inbox.reject": "Reject",
    "approvals.inbox.rejectFor": "Reject {summary}",
    "approvals.inbox.rejectRollback": "Reject and roll back",
    "approvals.inbox.rejectRollbackFor": "Reject and roll back {summary}",
    "approvals.inbox.resolveQuestion": "Resolve",
    "approvals.inbox.resolveQuestionFor": "Resolve {summary}",
    "approvals.inbox.answerFor": "Answer {summary}",
    "approvals.inbox.answerPlaceholder": "Answer or note the decision",
    "approvals.inbox.rollbackUnavailable": "Rollback unavailable",
    "approvals.inbox.checkpoint": "Checkpoint",
    "approvals.inbox.confirmTitle": "Reject and roll back",
    "approvals.inbox.confirmDescription": "Confirm the denial before restoring this checkpoint.",
    "approvals.inbox.confirmUnderstanding": "I understand rollback will restore this checkpoint",
    "approvals.inbox.confirmAction": "Confirm reject and roll back",
    "approvals.inbox.cancel": "Cancel",
    "approvals.inbox.kind": "Kind",
    "approvals.inbox.requested": "Requested",
    "approvals.inbox.requestedUnknown": "unknown",
    "approvals.inbox.select": "Select",
    "approvals.inbox.groupGoal": "Goal",
    "approvals.inbox.groupThread": "Thread",
    "approvals.inbox.status.approved": "Approved {approvalId}.",
    "approvals.inbox.status.denied": "Rejected {approvalId}.",
    "approvals.inbox.status.openQuestionResolved": "Resolved {approvalId}.",
    "approvals.inbox.status.batchApproved": "Approved {count} selected {approvalNoun}.",
    "approvals.inbox.status.batchPartialFailed": "Approved {completed} of {total} selected approvals. Stopped at {approvalId}: {error}",
    "approvals.inbox.status.rollbackUnavailable": "Rollback unavailable: {approvalId} has no checkpoint.",
    "approvals.inbox.status.rollbackRestored": "Rejected {approvalId} and restored checkpoint {checkpointId}.",
    "approvals.inbox.status.rollbackFailedAfterReject": "Rejected {approvalId}, but rollback failed: {error}",
    "chrome.sidebar.hideProjectSidebar": "Hide project sidebar",
    "chrome.sidebar.showProjectSidebar": "Show project sidebar",
    "chrome.sidebar.hideSidebar": "Hide sidebar",
    "chrome.sidebar.showSidebar": "Show sidebar",
    "chrome.sidebar.open": "Open sidebar",
    "chrome.sidebar.projectsAndSessions": "Projects and sessions",
    "chrome.sidebar.resizeProjectSidebar": "Resize project sidebar",
    "chrome.navigation.back": "Back",
    "chrome.navigation.forward": "Forward",
    "chrome.memory.open": "Open memory",
    "chrome.memory.title": "Memory",
    "chrome.memory.description": "Project memory governance",
    "chrome.inspector.open": "Open inspector",
    "chrome.inspector.title": "Inspector",
    "chrome.session.new": "New session",
    "sidebar.project": "Project",
    "sidebar.newChat": "New chat",
    "sidebar.search": "Search",
    "sidebar.projects": "Projects",
    "sidebar.addProject": "Add project",
    "sidebar.openSettings": "Open settings",
    "sidebar.settings": "Settings",
    "sidebar.newSession": "New session",
    "sidebar.newSessionFor": "New session for {project}",
    "sidebar.projectActionsFor": "Project actions for {project}",
    "sidebar.sessionActionsFor": "Session actions for {session}",
    "sidebar.forkedSessionLabel": "Forked session {session}, {fork}",
    "sidebar.archiveSession": "Archive session",
    "sidebar.archiveSessionFor": "Archive {session}",
    "sidebar.renameSession": "Rename session",
    "sidebar.pinSession": "Pin session",
    "sidebar.unpinSession": "Unpin session",
    "sidebar.compareWithParent": "Compare with parent",
    "sidebar.pinProject": "Pin project",
    "sidebar.unpinProject": "Unpin project",
    "sidebar.showInFinder": "Show in Finder",
    "sidebar.createPermanentWorktree": "Create permanent worktree",
    "sidebar.renameProject": "Rename project",
    "sidebar.markAllRead": "Mark all as read",
    "sidebar.archiveConversations": "Archive conversations",
    "sidebar.archiveProject": "Archive project",
    "sidebar.removeFromSidebar": "Remove from sidebar",
    "sidebar.loadingSessions": "Loading sessions...",
    "sidebar.noSessions": "No sessions",
    "sidebar.searchDialog.title": "Search sessions",
    "sidebar.searchDialog.description": "Find recent sessions by name.",
    "sidebar.searchDialog.placeholder": "Search sessions",
    "sidebar.searchDialog.noProject": "No project",
    "sidebar.searchDialog.noMatches": "No matching sessions",
    "sidebar.renameSession.title": "Rename session",
    "sidebar.renameSession.description": "Set a local title for this project's session list.",
    "sidebar.renameSession.field": "Session title",
    "sidebar.renameProject.title": "Rename project",
    "sidebar.renameProject.description": "Set a local name for this sidebar project.",
    "sidebar.renameProject.field": "Project name",
    "sidebar.confirm.archiveProject.title": "Archive {project}?",
    "sidebar.confirm.archiveProject.description": "This hides the project from the sidebar. It does not delete the folder or conversation files.",
    "sidebar.confirm.archiveProject.action": "Archive project",
    "sidebar.confirm.archiveConversations.title": "Archive conversations in {project}?",
    "sidebar.confirm.archiveConversations.description": "This hides this project's sessions from the default list. Runtime rollout files stay on disk.",
    "sidebar.confirm.archiveConversations.action": "Archive conversations",
    "sidebar.confirm.removeProject.title": "Remove {project} from the sidebar?",
    "sidebar.confirm.removeProject.description": "This removes the project from the desktop registry only. It does not delete files from disk.",
    "sidebar.confirm.removeProject.action": "Remove from sidebar",
    "chat.inspector": "Inspector",
    "chat.compare.sharedTurnSingular": "1 shared turn",
    "chat.compare.sharedTurnPlural": "{count} shared turns",
    "chat.compare.close": "Close branch compare",
    "chat.compare.parentLabel": "Parent branch transcript",
    "chat.compare.parentEyebrow": "Parent branch",
    "chat.compare.forkLabel": "Fork branch transcript",
    "chat.compare.forkEyebrow": "Fork branch",
    "chat.compare.empty": "No post-fork turns in this branch.",
    "chat.empty.title": "What should we build in {project}?",
    "chat.empty.addProject.title": "Add a project",
    "chat.empty.addProject.description": "Choose a folder before starting a session.",
    "chat.empty.addProject.action": "Add project",
    "chat.prompt.buildFeature.title": "Build a feature",
    "chat.prompt.buildFeature.description": "Describe the product behavior you want",
    "chat.prompt.buildFeature.value": "Build ",
    "chat.prompt.fixProblem.title": "Fix a problem",
    "chat.prompt.fixProblem.description": "Point ExAgent at a bug or rough edge",
    "chat.prompt.fixProblem.value": "Fix ",
    "chat.prompt.reviewCode.title": "Review the code",
    "chat.prompt.reviewCode.description": "Ask for risks, regressions, and tests",
    "chat.prompt.reviewCode.value": "Review ",
    "status.session.idle": "idle",
    "status.session.running": "running",
    "status.session.awaitingApproval": "awaiting approval",
    "status.session.failed": "failed",
    "status.session.archived": "archived",
    "status.agent.running": "running",
    "status.agent.spawning": "spawning",
    "status.agent.waitingApproval": "needs approval",
    "status.agent.done": "done",
    "status.agent.idle": "idle",
    "status.agent.failed": "failed",
    "inspector.sections.progress": "Progress",
    "inspector.sections.agents": "Agents",
    "inspector.sections.environment": "Environment",
    "inspector.sections.runtime": "Runtime",
    "inspector.sections.tokenUsage": "Token Usage",
    "inspector.sections.contextWindow": "Context Window",
    "inspector.sections.changedFiles": "Changed Files",
    "inspector.sections.events": "Events",
    "inspector.agentSummary.running": "{count} running",
    "inspector.agentSummary.singular": "1 agent",
    "inspector.agentSummary.plural": "{count} agents",
    "inspector.enabledCount": "{count} enabled",
    "inspector.context.percentUsed": "{percent}% used",
    "inspector.context.notReported": "not reported",
    "inspector.context.empty": "No context window reported for this thread.",
    "inspector.changedFiles.none": "none",
    "inspector.changedFiles.changed": "{count} changed",
    "inspector.changedFiles.empty": "No changed files reported.",
    "inspector.events.recorded": "{count} recorded",
    "inspector.events.empty": "No runtime events yet.",
    "inspector.waitingApproval.approvalSingular": "approval",
    "inspector.waitingApproval.approvalPlural": "approvals",
    "inspector.waitingApproval.expandSingular": "Expand 1 waiting approval agent",
    "inspector.waitingApproval.expandPlural": "Expand {count} waiting approval agents",
    "agents.treeLabel": "Running agents",
    "agents.expand": "Expand {name}",
    "agents.collapse": "Collapse {name}",
    "agents.openThread": "Open {name} agent thread, {details}",
    "agents.inspect": "Inspect {name}",
    "agents.activity": "activity",
    "agents.tool": "tool",
    "agents.tokens": "tokens",
    "tokenUsage.empty": "No token usage reported for this thread.",
    "tokenUsage.threadTotal": "thread total",
    "tokenUsage.input": "input",
    "tokenUsage.output": "output",
    "tokenUsage.reasoning": "reasoning",
    "tokenUsage.cachedInput": "cached input",
    "tokenUsage.lastTurn": "last turn",
    "tokenUsage.lastInput": "last input",
    "tokenUsage.lastOutput": "last output",
    "tokenUsage.summary.notReported": "not reported",
    "tokenUsage.summary.tokens": "{count} tokens",
    "goal.action": "Goal",
    "goal.objective": "Goal objective",
    "goal.tokenBudget": "Goal token budget",
    "goal.cancelEdit": "Cancel goal edit",
    "goal.save": "Save goal",
    "goal.pause": "Pause goal",
    "goal.resume": "Resume goal",
    "goal.edit": "Edit goal",
    "goal.clear": "Clear goal",
    "goal.mode": "Goal mode",
    "goal.modeFor": "Goal mode {mode}",
    "goal.mode.standard": "Standard",
    "goal.mode.reviewed": "Reviewed",
    "goal.mode.intensive": "Intensive",
    "goal.mode.standardTitle": "Standard goal mode",
    "goal.mode.reviewedTitle": "Reviewer-gated goal mode",
    "goal.mode.intensiveTitle": "Intensive reviewer-gated goal mode",
    "goal.status.draft": "draft",
    "goal.status.active": "active",
    "goal.status.complete": "complete",
    "goal.status.blocked": "blocked",
    "goal.status.budgetLimited": "budget limited",
    "goal.status.usageLimited": "usage limited",
    "goal.status.paused": "paused",
    "goal.usage.tokens": "{count} tokens",
    "goal.usage.left": "{remaining}/{budget} left",
    "transcript.actions.copyReply": "Copy reply",
    "transcript.actions.copiedReply": "Copied",
    "transcript.actions.forkFromHere": "Fork from here",
    "transcript.actions.forkFromReply": "Fork from this reply",
    "sessions.forkedFromTurn": "forked from turn {turn}",
    "composer.aria.promptComposer": "Prompt composer",
    "composer.aria.message": "Message ExAgent",
    "composer.aria.openActions": "Open composer actions",
    "composer.placeholder.hero": "Ask ExAgent to build, fix, or explain...",
    "composer.placeholder.dock": "Message ExAgent",
    "composer.plan.enabled": "Plan mode enabled",
    "composer.plan.short": "Plan",
    "composer.runtimePreset": "Runtime preset",
    "composer.runtimePreset.default": "Build",
    "composer.model.choose": "Choose model",
    "composer.model.button": "Composer model",
    "composer.model.search": "Search models",
    "composer.model.noModels": "No models found",
    "composer.model.configured": "Configured",
    "composer.model.selected": "Selected",
    "composer.model.providerDefault": "Provider default",
    "composer.model.available": "Available",
    "composer.thinking.mode": "Thinking mode",
    "composer.thinking.default": "Default",
    "composer.thinking.defaultAria": "Thinking default",
    "composer.thinking.off": "Off",
    "composer.thinking.offAria": "Thinking off",
    "composer.thinking.minimal": "Minimal",
    "composer.thinking.minimalAria": "Thinking minimal",
    "composer.thinking.low": "Low",
    "composer.thinking.lowAria": "Thinking low",
    "composer.thinking.medium": "Medium",
    "composer.thinking.mediumAria": "Thinking medium",
    "composer.thinking.high": "High",
    "composer.thinking.highAria": "Thinking high",
    "composer.thinking.xHigh": "XHigh",
    "composer.thinking.xHighAria": "Thinking xhigh",
    "composer.slash.label": "Slash commands",
    "composer.slash.compact.label": "Compact conversation",
    "composer.slash.compact.detail": "Summarize history and shrink context",
    "composer.slash.compact.busyDetail": "Available when the thread is idle",
    "composer.slash.goal.label": "Set goal",
    "composer.slash.goal.detail": "Create or edit the thread goal",
    "composer.slash.plan.enable": "Enable plan mode",
    "composer.slash.plan.disable": "Disable plan mode",
    "composer.slash.plan.detail": "Toggle planning for the next prompt",
    "composer.send": "Send",
    "composer.interrupt": "Interrupt",
    "composer.actions.addPhotosAndFiles": "Add photos",
    "composer.actions.attachChrome": "Attach Google Chrome",
    "composer.actions.planMode": "Plan mode",
    "composer.actions.goal": "Goal",
    "composer.actions.plugins": "Plugins",
    "composer.attachments.imageInputUnavailable": "Selected model accepts text only. Remove photos or choose a vision-capable model.",
    "composer.attachments.selectedImages": "Selected photos",
    "composer.attachments.removeFor": "Remove {name}",
    "composer.attachments.textOnly": "Text only",
    "composer.model.configureProvider": "Configure provider",
    "composer.model.configureProviderDescription": "Configure a provider to choose a model.",
    "settings.title": "Settings",
    "settings.description": "Configure ExAgent runtime behavior.",
    "settings.sections.general": "General",
    "settings.sections.providers": "Providers",
    "settings.sections.mcp": "MCP",
    "settings.sections.skills": "Skills",
    "settings.sections.webSearch": "Web search",
    "settings.sections.archive": "Archive",
    "settings.sections.aria": "Settings sections",
    "settings.general.title": "General",
    "settings.general.description": "Set app-wide preferences for the desktop workbench.",
    "settings.theme.title": "Theme",
    "settings.theme.description": "Choose how ExAgent should render its interface colors.",
    "settings.theme.current": "Current theme",
    "settings.theme.system.title": "System",
    "settings.theme.system.description": "Follow the current macOS appearance.",
    "settings.theme.light.title": "Light",
    "settings.theme.light.description": "Use the light ExAgent interface.",
    "settings.theme.dark.title": "Dark",
    "settings.theme.dark.description": "Use the dark ExAgent interface.",
    "settings.theme.note": "The theme preference is saved locally and applies immediately.",
    "settings.language.title": "Language",
    "settings.language.description": "Choose the interface language for this desktop app.",
    "settings.language.current": "Current language",
    "settings.language.english.title": "English",
    "settings.language.english.description": "Use English labels, controls, and settings copy.",
    "settings.language.chinese.title": "中文",
    "settings.language.chinese.description": "使用中文显示界面标签、控件与设置说明。",
    "settings.language.note": "The language preference is saved locally and applies immediately.",
    "settings.providers.title": "Providers",
    "settings.providers.description": "Connect a provider for new ExAgent runtime sessions.",
    "settings.providers.popular": "Popular",
    "settings.providers.loading": "Loading providers...",
    "settings.providers.back": "Back to providers",
    "settings.providers.connect": "Connect",
    "settings.providers.configure": "Configure",
    "settings.providers.recommended": "Recommended",
    "settings.providers.active": "Active",
    "settings.providers.planned": "Planned",
    "settings.providers.descriptions.openai": "Use ChatGPT Pro/Plus or an API key",
    "settings.providers.descriptions.openaiCompatible": "Use OpenRouter, DeepSeek, local gateways, or another compatible endpoint",
    "settings.providers.descriptions.anthropic": "Use Claude Pro/Max or an API key",
    "settings.providers.descriptions.githubCopilot": "Use GitHub Copilot with device OAuth",
    "settings.providers.descriptions.google": "Use Gemini models with a Google API key",
    "settings.providers.descriptions.deepseek": "Use DeepSeek API with an API key",
    "settings.providers.descriptions.kimi": "Use Kimi API with a Moonshot API key",
    "settings.providers.descriptions.glm": "Use GLM API with a Zhipu API key",
    "settings.connection.chooseOpenAi": "Choose an OpenAI sign-in method.",
    "settings.connection.openAiAuthMode": "OpenAI auth mode",
    "settings.connection.openAiApiKey": "OpenAI API key",
    "settings.connection.apiKey": "API key",
    "settings.connection.savedInKeychain": "Saved",
    "settings.connection.compatibleDescription": "Enter the OpenAI-compatible endpoint for this provider, with an optional API key.",
    "settings.connection.apiKeyDescription": "Enter your API key to connect this provider.",
    "settings.connection.baseUrl": "Base URL",
    "settings.connection.baseUrlPresets": "Presets",
    "settings.connection.baseUrlPresetsAria": "Base URL presets",
    "settings.connection.baseUrlPresetInternational": "International",
    "settings.connection.baseUrlPresetMainlandChina": "Mainland China",
    "settings.connection.model": "Model",
    "settings.connection.discovering": "Discovering",
    "settings.connection.discoverModels": "Discover models",
    "settings.connection.noModels": "No models returned.",
    "settings.connection.clearSavedApiKey": "Clear saved API key",
    "settings.connection.saveProvider": "Save provider",
    "settings.connection.saving": "Saving",
    "settings.connection.testConnection": "Test connection",
    "settings.connection.testing": "Testing",
    "settings.connection.startOAuth": "Start OAuth login",
    "settings.connection.startingOAuth": "Starting OAuth",
    "settings.connection.completeOAuth": "Complete OAuth login",
    "settings.connection.completingOAuth": "Completing OAuth",
    "settings.connection.oauthCode": "Verification code",
    "settings.connection.openOAuthPage": "Open verification page",
    "settings.connection.plannedChatGpt": "ChatGPT account login is planned for a later desktop auth phase.",
    "settings.connection.plannedProvider": "Enter your API key to connect this provider when support is available.",
    "settings.connection.comingSoon": "Coming soon",
    "settings.connection.githubDeployment": "Select GitHub deployment type",
    "settings.connection.githubPublic": "Public",
    "settings.connection.githubEnterpriseDescription": "Data residency or self-hosted",
    "settings.mcp.title": "MCP",
    "settings.mcp.description": "Configure local MCP servers exposed to the desktop runtime.",
    "settings.mcp.add": "Add MCP server",
    "settings.mcp.empty": "No MCP servers configured.",
    "settings.mcp.save": "Save MCP",
    "settings.mcp.newName": "New MCP server",
    "settings.skills.title": "Skills",
    "settings.skills.description": "Register skill roots that should be available in desktop sessions.",
    "settings.skills.add": "Add skill root",
    "settings.skills.empty": "No skill roots configured.",
    "settings.skills.save": "Save skills",
    "settings.skills.newName": "New skill root",
    "settings.archive.title": "Archive",
    "settings.archive.description": "Restore archived conversations from visible projects.",
    "settings.archive.refresh": "Refresh",
    "settings.archive.loading": "Loading archived conversations...",
    "settings.archive.emptyTitle": "No archived conversations",
    "settings.archive.emptyDescription": "Archived projects are not shown here. Add the folder again to restore a project.",
    "settings.archive.archived": "archived",
    "settings.archive.restore": "Restore",
    "settings.archive.open": "Open",
    "common.enabled": "Enabled",
    "common.remove": "Remove",
    "common.name": "Name",
    "common.command": "Command",
    "common.arguments": "Arguments",
    "common.workingDirectory": "Working directory",
    "common.environment": "Environment",
    "common.scope": "Scope",
    "common.path": "Path",
    "common.saving": "Saving",
    "common.current": "Current",
    "common.cancel": "Cancel",
    "common.save": "Save",
    "common.close": "Close"
  },
  zh: {
    "approvals.inbox.title": "审批收件箱",
    "approvals.inbox.description": "查看当前线程中的待处理审批。",
    "approvals.inbox.empty": "没有待处理审批。",
    "approvals.inbox.pending": "待处理",
    "approvals.inbox.approvalSingular": "审批",
    "approvals.inbox.approvalPlural": "审批",
    "approvals.inbox.loading": "正在刷新审批...",
    "approvals.inbox.approveSelected": "批准所选",
    "approvals.inbox.approveSelectedAria": "批准所选审批",
    "approvals.inbox.clearSelection": "清除选择",
    "approvals.inbox.details": "显示详情",
    "approvals.inbox.detailsFor": "显示 {summary} 的详情",
    "approvals.inbox.hideDetails": "隐藏详情",
    "approvals.inbox.hideDetailsFor": "隐藏 {summary} 的详情",
    "approvals.inbox.approve": "批准",
    "approvals.inbox.approveFor": "批准 {summary}",
    "approvals.inbox.reject": "拒绝",
    "approvals.inbox.rejectFor": "拒绝 {summary}",
    "approvals.inbox.rejectRollback": "拒绝并回滚",
    "approvals.inbox.rejectRollbackFor": "拒绝并回滚 {summary}",
    "approvals.inbox.resolveQuestion": "解决",
    "approvals.inbox.resolveQuestionFor": "解决 {summary}",
    "approvals.inbox.answerFor": "回答 {summary}",
    "approvals.inbox.answerPlaceholder": "填写答案或决策备注",
    "approvals.inbox.rollbackUnavailable": "无法回滚",
    "approvals.inbox.checkpoint": "检查点",
    "approvals.inbox.confirmTitle": "拒绝并回滚",
    "approvals.inbox.confirmDescription": "恢复此检查点前请确认拒绝操作。",
    "approvals.inbox.confirmUnderstanding": "我了解回滚会恢复此检查点",
    "approvals.inbox.confirmAction": "确认拒绝并回滚",
    "approvals.inbox.cancel": "取消",
    "approvals.inbox.kind": "类型",
    "approvals.inbox.requested": "请求时间",
    "approvals.inbox.requestedUnknown": "未知",
    "approvals.inbox.select": "选择",
    "approvals.inbox.groupGoal": "目标",
    "approvals.inbox.groupThread": "线程",
    "approvals.inbox.status.approved": "已批准 {approvalId}。",
    "approvals.inbox.status.denied": "已拒绝 {approvalId}。",
    "approvals.inbox.status.openQuestionResolved": "已解决 {approvalId}。",
    "approvals.inbox.status.batchApproved": "已批准 {count} 个所选审批。",
    "approvals.inbox.status.batchPartialFailed": "已批准 {completed}/{total} 个所选审批。停在 {approvalId}：{error}",
    "approvals.inbox.status.rollbackUnavailable": "无法回滚：{approvalId} 没有检查点。",
    "approvals.inbox.status.rollbackRestored": "已拒绝 {approvalId} 并恢复检查点 {checkpointId}。",
    "approvals.inbox.status.rollbackFailedAfterReject": "已拒绝 {approvalId}，但回滚失败：{error}",
    "chrome.sidebar.hideProjectSidebar": "隐藏项目侧栏",
    "chrome.sidebar.showProjectSidebar": "显示项目侧栏",
    "chrome.sidebar.hideSidebar": "隐藏侧栏",
    "chrome.sidebar.showSidebar": "显示侧栏",
    "chrome.sidebar.open": "打开侧栏",
    "chrome.sidebar.projectsAndSessions": "项目和会话",
    "chrome.sidebar.resizeProjectSidebar": "调整项目侧栏宽度",
    "chrome.navigation.back": "返回",
    "chrome.navigation.forward": "前进",
    "chrome.memory.open": "打开记忆",
    "chrome.memory.title": "记忆",
    "chrome.memory.description": "项目记忆治理",
    "chrome.inspector.open": "打开检查器",
    "chrome.inspector.title": "检查器",
    "chrome.session.new": "新会话",
    "sidebar.project": "项目",
    "sidebar.newChat": "新对话",
    "sidebar.search": "搜索",
    "sidebar.projects": "项目",
    "sidebar.addProject": "添加项目",
    "sidebar.openSettings": "打开设置",
    "sidebar.settings": "设置",
    "sidebar.newSession": "新建会话",
    "sidebar.newSessionFor": "为 {project} 新建会话",
    "sidebar.projectActionsFor": "{project} 的项目操作",
    "sidebar.sessionActionsFor": "{session} 的会话操作",
    "sidebar.forkedSessionLabel": "分叉会话 {session}，{fork}",
    "sidebar.archiveSession": "归档会话",
    "sidebar.archiveSessionFor": "归档 {session}",
    "sidebar.renameSession": "重命名会话",
    "sidebar.pinSession": "置顶会话",
    "sidebar.unpinSession": "取消置顶会话",
    "sidebar.compareWithParent": "与父分支对比",
    "sidebar.pinProject": "置顶项目",
    "sidebar.unpinProject": "取消置顶项目",
    "sidebar.showInFinder": "在 Finder 中显示",
    "sidebar.createPermanentWorktree": "创建永久 worktree",
    "sidebar.renameProject": "重命名项目",
    "sidebar.markAllRead": "全部标为已读",
    "sidebar.archiveConversations": "归档对话",
    "sidebar.archiveProject": "归档项目",
    "sidebar.removeFromSidebar": "从侧栏移除",
    "sidebar.loadingSessions": "正在加载会话...",
    "sidebar.noSessions": "没有会话",
    "sidebar.searchDialog.title": "搜索会话",
    "sidebar.searchDialog.description": "按名称查找最近的会话。",
    "sidebar.searchDialog.placeholder": "搜索会话",
    "sidebar.searchDialog.noProject": "未指定项目",
    "sidebar.searchDialog.noMatches": "没有匹配的会话",
    "sidebar.renameSession.title": "重命名会话",
    "sidebar.renameSession.description": "为此项目的会话列表设置本地标题。",
    "sidebar.renameSession.field": "会话标题",
    "sidebar.renameProject.title": "重命名项目",
    "sidebar.renameProject.description": "为侧栏中的此项目设置本地名称。",
    "sidebar.renameProject.field": "项目名称",
    "sidebar.confirm.archiveProject.title": "归档 {project}？",
    "sidebar.confirm.archiveProject.description": "这会从侧栏隐藏项目，不会删除文件夹或会话文件。",
    "sidebar.confirm.archiveProject.action": "归档项目",
    "sidebar.confirm.archiveConversations.title": "归档 {project} 中的对话？",
    "sidebar.confirm.archiveConversations.description": "这会从默认列表隐藏此项目的会话，运行时 rollout 文件仍保留在磁盘上。",
    "sidebar.confirm.archiveConversations.action": "归档对话",
    "sidebar.confirm.removeProject.title": "从侧栏移除 {project}？",
    "sidebar.confirm.removeProject.description": "这只会从桌面项目注册表移除该项目，不会删除磁盘上的文件。",
    "sidebar.confirm.removeProject.action": "从侧栏移除",
    "chat.inspector": "检查器",
    "chat.compare.sharedTurnSingular": "1 轮共享对话",
    "chat.compare.sharedTurnPlural": "{count} 轮共享对话",
    "chat.compare.close": "关闭分支对比",
    "chat.compare.parentLabel": "父分支转录",
    "chat.compare.parentEyebrow": "父分支",
    "chat.compare.forkLabel": "分叉分支转录",
    "chat.compare.forkEyebrow": "分叉分支",
    "chat.compare.empty": "此分支没有分叉后的对话。",
    "chat.empty.title": "要在 {project} 里构建什么？",
    "chat.empty.addProject.title": "添加项目",
    "chat.empty.addProject.description": "开始会话前先选择一个文件夹。",
    "chat.empty.addProject.action": "添加项目",
    "chat.prompt.buildFeature.title": "构建功能",
    "chat.prompt.buildFeature.description": "描述你想要的产品行为",
    "chat.prompt.buildFeature.value": "构建 ",
    "chat.prompt.fixProblem.title": "修复问题",
    "chat.prompt.fixProblem.description": "指出 bug 或体验粗糙处",
    "chat.prompt.fixProblem.value": "修复 ",
    "chat.prompt.reviewCode.title": "审查代码",
    "chat.prompt.reviewCode.description": "检查风险、回归和测试",
    "chat.prompt.reviewCode.value": "审查 ",
    "status.session.idle": "空闲",
    "status.session.running": "运行中",
    "status.session.awaitingApproval": "等待审批",
    "status.session.failed": "失败",
    "status.session.archived": "已归档",
    "status.agent.running": "运行中",
    "status.agent.spawning": "创建中",
    "status.agent.waitingApproval": "需要审批",
    "status.agent.done": "完成",
    "status.agent.idle": "空闲",
    "status.agent.failed": "失败",
    "inspector.sections.progress": "进度",
    "inspector.sections.agents": "Agents",
    "inspector.sections.environment": "环境",
    "inspector.sections.runtime": "运行时",
    "inspector.sections.tokenUsage": "Token 用量",
    "inspector.sections.contextWindow": "上下文窗口",
    "inspector.sections.changedFiles": "变更文件",
    "inspector.sections.events": "事件",
    "inspector.agentSummary.running": "{count} 个运行中",
    "inspector.agentSummary.singular": "1 个 agent",
    "inspector.agentSummary.plural": "{count} 个 agent",
    "inspector.enabledCount": "已启用 {count} 个",
    "inspector.context.percentUsed": "已用 {percent}%",
    "inspector.context.notReported": "未报告",
    "inspector.context.empty": "此线程未报告上下文窗口。",
    "inspector.changedFiles.none": "无",
    "inspector.changedFiles.changed": "{count} 个变更",
    "inspector.changedFiles.empty": "没有报告变更文件。",
    "inspector.events.recorded": "{count} 条记录",
    "inspector.events.empty": "还没有运行时事件。",
    "inspector.waitingApproval.approvalSingular": "审批",
    "inspector.waitingApproval.approvalPlural": "审批",
    "inspector.waitingApproval.expandSingular": "展开 1 个等待审批的 agent",
    "inspector.waitingApproval.expandPlural": "展开 {count} 个等待审批的 agent",
    "agents.treeLabel": "运行中的 agents",
    "agents.expand": "展开 {name}",
    "agents.collapse": "收起 {name}",
    "agents.openThread": "打开 {name} 的 agent 线程，{details}",
    "agents.inspect": "检查 {name}",
    "agents.activity": "活动",
    "agents.tool": "工具",
    "agents.tokens": "tokens",
    "tokenUsage.empty": "此线程未报告 token 用量。",
    "tokenUsage.threadTotal": "线程总计",
    "tokenUsage.input": "输入",
    "tokenUsage.output": "输出",
    "tokenUsage.reasoning": "推理",
    "tokenUsage.cachedInput": "缓存输入",
    "tokenUsage.lastTurn": "最近一轮",
    "tokenUsage.lastInput": "最近输入",
    "tokenUsage.lastOutput": "最近输出",
    "tokenUsage.summary.notReported": "未报告",
    "tokenUsage.summary.tokens": "{count} tokens",
    "goal.action": "目标",
    "goal.objective": "目标内容",
    "goal.tokenBudget": "目标 token 预算",
    "goal.cancelEdit": "取消编辑目标",
    "goal.save": "保存目标",
    "goal.pause": "暂停目标",
    "goal.resume": "继续目标",
    "goal.edit": "编辑目标",
    "goal.clear": "清除目标",
    "goal.mode": "目标模式",
    "goal.modeFor": "目标模式 {mode}",
    "goal.mode.standard": "标准",
    "goal.mode.reviewed": "需审查",
    "goal.mode.intensive": "强化",
    "goal.mode.standardTitle": "标准目标模式",
    "goal.mode.reviewedTitle": "审查门控目标模式",
    "goal.mode.intensiveTitle": "强化审查门控目标模式",
    "goal.status.draft": "草稿",
    "goal.status.active": "进行中",
    "goal.status.complete": "完成",
    "goal.status.blocked": "阻塞",
    "goal.status.budgetLimited": "预算受限",
    "goal.status.usageLimited": "用量受限",
    "goal.status.paused": "已暂停",
    "goal.usage.tokens": "{count} tokens",
    "goal.usage.left": "剩余 {remaining}/{budget}",
    "transcript.actions.copyReply": "复制回复",
    "transcript.actions.copiedReply": "已复制",
    "transcript.actions.forkFromHere": "从这里分叉",
    "transcript.actions.forkFromReply": "从这条回复分叉",
    "sessions.forkedFromTurn": "从第 {turn} 轮分叉",
    "composer.aria.promptComposer": "提示词输入区",
    "composer.aria.message": "输入给 ExAgent 的消息",
    "composer.aria.openActions": "打开输入区操作",
    "composer.placeholder.hero": "让 ExAgent 构建、修复或解释...",
    "composer.placeholder.dock": "输入给 ExAgent 的消息",
    "composer.plan.enabled": "计划模式已启用",
    "composer.plan.short": "计划",
    "composer.runtimePreset": "运行预设",
    "composer.runtimePreset.default": "构建",
    "composer.model.choose": "选择模型",
    "composer.model.button": "输入区模型",
    "composer.model.search": "搜索模型",
    "composer.model.noModels": "没有找到模型",
    "composer.model.configured": "已配置",
    "composer.model.selected": "已选择",
    "composer.model.providerDefault": "提供方默认",
    "composer.model.available": "可用",
    "composer.thinking.mode": "思考模式",
    "composer.thinking.default": "默认",
    "composer.thinking.defaultAria": "默认思考",
    "composer.thinking.off": "关闭",
    "composer.thinking.offAria": "关闭思考",
    "composer.thinking.minimal": "极简",
    "composer.thinking.minimalAria": "极简思考",
    "composer.thinking.low": "低",
    "composer.thinking.lowAria": "低强度思考",
    "composer.thinking.medium": "中",
    "composer.thinking.mediumAria": "中等思考",
    "composer.thinking.high": "高",
    "composer.thinking.highAria": "高强度思考",
    "composer.thinking.xHigh": "超高",
    "composer.thinking.xHighAria": "超高强度思考",
    "composer.slash.label": "斜杠菜单",
    "composer.slash.compact.label": "压缩对话",
    "composer.slash.compact.detail": "总结历史并缩小上下文",
    "composer.slash.compact.busyDetail": "线程空闲时可用",
    "composer.slash.goal.label": "设置目标",
    "composer.slash.goal.detail": "创建或编辑线程目标",
    "composer.slash.plan.enable": "启用计划模式",
    "composer.slash.plan.disable": "关闭计划模式",
    "composer.slash.plan.detail": "切换下一条提示词的计划模式",
    "composer.send": "发送",
    "composer.interrupt": "中断",
    "composer.actions.addPhotosAndFiles": "添加照片",
    "composer.actions.attachChrome": "附加 Google Chrome",
    "composer.actions.planMode": "计划模式",
    "composer.actions.goal": "追求目标",
    "composer.actions.plugins": "插件",
    "composer.attachments.imageInputUnavailable": "当前模型只支持文本。请移除图片或切换到支持视觉输入的模型。",
    "composer.attachments.selectedImages": "已选择的照片",
    "composer.attachments.removeFor": "移除 {name}",
    "composer.attachments.textOnly": "仅文本",
    "composer.model.configureProvider": "请配置 Provider",
    "composer.model.configureProviderDescription": "请先配置 Provider 后再选择模型。",
    "settings.title": "设置",
    "settings.description": "配置 ExAgent 的运行行为。",
    "settings.sections.general": "通用",
    "settings.sections.providers": "模型提供方",
    "settings.sections.mcp": "MCP",
    "settings.sections.skills": "技能",
    "settings.sections.webSearch": "网页搜索",
    "settings.sections.archive": "归档",
    "settings.sections.aria": "设置分区",
    "settings.general.title": "通用",
    "settings.general.description": "设置此桌面工作台的全局偏好。",
    "settings.theme.title": "主题",
    "settings.theme.description": "选择 ExAgent 界面的颜色外观。",
    "settings.theme.current": "当前主题",
    "settings.theme.system.title": "跟随系统",
    "settings.theme.system.description": "跟随当前 macOS 外观。",
    "settings.theme.light.title": "浅色",
    "settings.theme.light.description": "使用 ExAgent 浅色界面。",
    "settings.theme.dark.title": "深色",
    "settings.theme.dark.description": "使用 ExAgent 深色界面。",
    "settings.theme.note": "主题偏好会保存在本机，并立即应用。",
    "settings.language.title": "语言",
    "settings.language.description": "选择此桌面应用的界面语言。",
    "settings.language.current": "当前语言",
    "settings.language.english.title": "English",
    "settings.language.english.description": "使用英文显示界面标签、控件与设置说明。",
    "settings.language.chinese.title": "中文",
    "settings.language.chinese.description": "使用中文显示界面标签、控件与设置说明。",
    "settings.language.note": "语言偏好会保存在本机，并立即应用。",
    "settings.providers.title": "模型提供方",
    "settings.providers.description": "连接用于新 ExAgent 运行会话的模型提供方。",
    "settings.providers.popular": "热门",
    "settings.providers.loading": "正在加载模型提供方...",
    "settings.providers.back": "返回模型提供方",
    "settings.providers.connect": "连接",
    "settings.providers.configure": "配置",
    "settings.providers.recommended": "推荐",
    "settings.providers.active": "已启用",
    "settings.providers.planned": "计划中",
    "settings.providers.descriptions.openai": "使用 ChatGPT Pro/Plus 或 API 密钥连接",
    "settings.providers.descriptions.openaiCompatible": "使用 OpenRouter、DeepSeek、本地网关或其他兼容端点",
    "settings.providers.descriptions.anthropic": "使用 Claude Pro/Max 或 API 密钥连接",
    "settings.providers.descriptions.githubCopilot": "使用 GitHub Copilot 设备 OAuth 连接",
    "settings.providers.descriptions.google": "使用 Google API 密钥连接 Gemini 模型",
    "settings.providers.descriptions.deepseek": "使用 DeepSeek API 密钥连接",
    "settings.providers.descriptions.kimi": "使用 Moonshot API 密钥连接 Kimi",
    "settings.providers.descriptions.glm": "使用智谱 API 密钥连接 GLM",
    "settings.connection.chooseOpenAi": "选择 OpenAI 的登录方式。",
    "settings.connection.openAiAuthMode": "OpenAI 登录方式",
    "settings.connection.openAiApiKey": "OpenAI API 密钥",
    "settings.connection.apiKey": "API 密钥",
    "settings.connection.savedInKeychain": "已保存",
    "settings.connection.compatibleDescription": "输入此提供方的 OpenAI-compatible endpoint，可选择提供 API 密钥。",
    "settings.connection.apiKeyDescription": "输入你的 API 密钥以连接此提供方。",
    "settings.connection.baseUrl": "Base URL",
    "settings.connection.baseUrlPresets": "预设",
    "settings.connection.baseUrlPresetsAria": "Base URL 预设",
    "settings.connection.baseUrlPresetInternational": "国际平台",
    "settings.connection.baseUrlPresetMainlandChina": "国内平台",
    "settings.connection.model": "模型",
    "settings.connection.discovering": "正在发现",
    "settings.connection.discoverModels": "发现模型",
    "settings.connection.noModels": "没有返回模型。",
    "settings.connection.clearSavedApiKey": "清除已保存的 API 密钥",
    "settings.connection.saveProvider": "保存提供方",
    "settings.connection.saving": "正在保存",
    "settings.connection.testConnection": "测试连接",
    "settings.connection.testing": "正在测试",
    "settings.connection.startOAuth": "开始 OAuth 登录",
    "settings.connection.startingOAuth": "正在开始 OAuth",
    "settings.connection.completeOAuth": "完成 OAuth 登录",
    "settings.connection.completingOAuth": "正在完成 OAuth",
    "settings.connection.oauthCode": "验证码",
    "settings.connection.openOAuthPage": "打开验证页面",
    "settings.connection.plannedChatGpt": "ChatGPT 账户登录将在后续桌面认证阶段支持。",
    "settings.connection.plannedProvider": "此提供方支持完成后，可在这里输入 API 密钥连接账户。",
    "settings.connection.comingSoon": "即将支持",
    "settings.connection.githubDeployment": "选择 GitHub 部署类型",
    "settings.connection.githubPublic": "公共版",
    "settings.connection.githubEnterpriseDescription": "数据驻留或自托管",
    "settings.mcp.title": "MCP",
    "settings.mcp.description": "配置暴露给桌面运行时的本地 MCP 服务器。",
    "settings.mcp.add": "添加 MCP 服务器",
    "settings.mcp.empty": "尚未配置 MCP 服务器。",
    "settings.mcp.save": "保存 MCP",
    "settings.mcp.newName": "新的 MCP 服务器",
    "settings.skills.title": "技能",
    "settings.skills.description": "注册桌面会话可使用的技能根目录。",
    "settings.skills.add": "添加技能根目录",
    "settings.skills.empty": "尚未配置技能根目录。",
    "settings.skills.save": "保存技能",
    "settings.skills.newName": "新的技能根目录",
    "settings.archive.title": "归档",
    "settings.archive.description": "从可见项目中恢复已归档的对话。",
    "settings.archive.refresh": "刷新",
    "settings.archive.loading": "正在加载已归档对话...",
    "settings.archive.emptyTitle": "没有已归档对话",
    "settings.archive.emptyDescription": "已归档项目不会显示在这里。重新添加文件夹即可恢复项目。",
    "settings.archive.archived": "已归档",
    "settings.archive.restore": "恢复",
    "settings.archive.open": "打开",
    "common.enabled": "启用",
    "common.remove": "移除",
    "common.name": "名称",
    "common.command": "命令",
    "common.arguments": "参数",
    "common.workingDirectory": "工作目录",
    "common.environment": "环境变量",
    "common.scope": "范围",
    "common.path": "路径",
    "common.saving": "正在保存",
    "common.current": "当前",
    "common.cancel": "取消",
    "common.save": "保存",
    "common.close": "关闭"
  }
};

const defaultI18nContext: I18nContextValue = {
  locale: "en",
  setLocale: () => {},
  t: (key) => translations.en[key]
};

const I18nContext = createContext<I18nContextValue>(defaultI18nContext);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(() => readStoredLocale());

  useEffect(() => {
    document.documentElement.lang = locale === "zh" ? "zh-CN" : "en";
    window.localStorage.setItem(localeStorageKey, locale);
  }, [locale]);

  const value = useMemo<I18nContextValue>(
    () => ({
      locale,
      setLocale: setLocaleState,
      t: (key) => translations[locale][key]
    }),
    [locale]
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n() {
  return useContext(I18nContext);
}

function readStoredLocale(): Locale {
  if (typeof window === "undefined") {
    return "en";
  }
  const stored = window.localStorage.getItem(localeStorageKey);
  return stored === "zh" || stored === "en" ? stored : "en";
}
