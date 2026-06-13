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
  | "approvals.inbox.status.batchApproved"
  | "approvals.inbox.status.batchPartialFailed"
  | "approvals.inbox.status.rollbackUnavailable"
  | "approvals.inbox.status.rollbackRestored"
  | "approvals.inbox.status.rollbackFailedAfterReject"
  | "transcript.actions.forkFromHere"
  | "transcript.actions.forkFromReply"
  | "sessions.forkedFromTurn"
  | "composer.actions.addPhotosAndFiles"
  | "composer.actions.attachChrome"
  | "composer.actions.planMode"
  | "composer.actions.goal"
  | "composer.actions.plugins"
  | "composer.attachments.imageInputUnavailable"
  | "composer.attachments.selectedImages"
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
    "approvals.inbox.status.batchApproved": "Approved {count} selected {approvalNoun}.",
    "approvals.inbox.status.batchPartialFailed": "Approved {completed} of {total} selected approvals. Stopped at {approvalId}: {error}",
    "approvals.inbox.status.rollbackUnavailable": "Rollback unavailable: {approvalId} has no checkpoint.",
    "approvals.inbox.status.rollbackRestored": "Rejected {approvalId} and restored checkpoint {checkpointId}.",
    "approvals.inbox.status.rollbackFailedAfterReject": "Rejected {approvalId}, but rollback failed: {error}",
    "transcript.actions.forkFromHere": "Fork from here",
    "transcript.actions.forkFromReply": "Fork from this reply",
    "sessions.forkedFromTurn": "forked from turn {turn}",
    "composer.actions.addPhotosAndFiles": "Add photos",
    "composer.actions.attachChrome": "Attach Google Chrome",
    "composer.actions.planMode": "Plan mode",
    "composer.actions.goal": "Goal",
    "composer.actions.plugins": "Plugins",
    "composer.attachments.imageInputUnavailable": "Selected model accepts text only. Remove photos or choose a vision-capable model.",
    "composer.attachments.selectedImages": "Selected photos",
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
    "approvals.inbox.status.batchApproved": "已批准 {count} 个所选审批。",
    "approvals.inbox.status.batchPartialFailed": "已批准 {completed}/{total} 个所选审批。停在 {approvalId}：{error}",
    "approvals.inbox.status.rollbackUnavailable": "无法回滚：{approvalId} 没有检查点。",
    "approvals.inbox.status.rollbackRestored": "已拒绝 {approvalId} 并恢复检查点 {checkpointId}。",
    "approvals.inbox.status.rollbackFailedAfterReject": "已拒绝 {approvalId}，但回滚失败：{error}",
    "transcript.actions.forkFromHere": "从这里分叉",
    "transcript.actions.forkFromReply": "从这条回复分叉",
    "sessions.forkedFromTurn": "从第 {turn} 轮分叉",
    "composer.actions.addPhotosAndFiles": "添加照片",
    "composer.actions.attachChrome": "附加 Google Chrome",
    "composer.actions.planMode": "计划模式",
    "composer.actions.goal": "追求目标",
    "composer.actions.plugins": "插件",
    "composer.attachments.imageInputUnavailable": "当前模型只支持文本。请移除图片或切换到支持视觉输入的模型。",
    "composer.attachments.selectedImages": "已选择的照片",
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
