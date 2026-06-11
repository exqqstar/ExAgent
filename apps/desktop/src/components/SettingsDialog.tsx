import { useEffect, useMemo, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import {
  Archive,
  ArrowLeft,
  Check,
  ChevronDown,
  ExternalLink,
  KeyRound,
  Monitor,
  Moon,
  RefreshCw,
  Server,
  Settings2,
  Sparkles,
  Sun,
  type LucideIcon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ArchivedConversationsPanel } from "@/components/settings/ArchivedConversationsPanel";
import { McpSettingsPanel } from "@/components/settings/McpSettingsPanel";
import { SkillsSettingsPanel } from "@/components/settings/SkillsSettingsPanel";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  SettingsPanel,
  SettingsPanelHeader,
} from "@/components/settings/SettingsPanelPrimitives";
import { exagentClient } from "@/api/exagentClient";
import { useI18n, type Locale, type TranslationKey } from "@/lib/i18n";
import { useThemePreference, type ThemePreference } from "@/lib/theme";
import { applyProviderSettings } from "@/stores/workbenchStore";
import type {
  ProviderConnectionTestResponse,
  ProviderConfigView,
  ProviderDescriptor,
  ChatGptDeviceCode,
  GitHubCopilotDeviceCode,
  ProviderModelListResponse,
  ProviderModelView,
  ProviderSettingsResponse,
} from "@/types";
import { cn } from "@/lib/utils";

const defaultBaseUrl = "https://api.openai.com/v1";
const defaultModel = "gpt-5.5";

type ScopedModelDiscoveryResult = {
  requestKey: string;
  result: ProviderModelListResponse;
};

type ScopedConnectionResult = {
  requestKey: string;
  result: ProviderConnectionTestResponse;
};

type OpenAiAuthMode = "headless" | "api_key";
type OAuthDeviceState =
  | { providerId: "openai"; device: ChatGptDeviceCode }
  | { providerId: "github_copilot"; device: GitHubCopilotDeviceCode };
type SettingsSectionId =
  | "general"
  | "providers"
  | "mcp"
  | "skills"
  | "archive";
type ProviderPreset = {
  key: string;
  providerId: string;
  name: string;
  description: string;
  mark: string;
  recommended?: boolean;
  defaultBaseUrl?: string;
  defaultModel?: string;
};

type BaseUrlPreset = {
  labelKey: TranslationKey;
  url: string;
};

const settingsSections = [
  { id: "general", labelKey: "settings.sections.general", icon: Settings2 },
  { id: "providers", labelKey: "settings.sections.providers", icon: KeyRound },
  { id: "mcp", labelKey: "settings.sections.mcp", icon: Server },
  { id: "skills", labelKey: "settings.sections.skills", icon: Sparkles },
  { id: "archive", labelKey: "settings.sections.archive", icon: Archive },
] as const;

const openAiAuthModeOptions = [
  { id: "headless", title: "ChatGPT Pro/Plus (headless)" },
  { id: "api_key", titleKey: "settings.connection.apiKey" },
] as const satisfies ReadonlyArray<{
  id: OpenAiAuthMode;
  title?: string;
  titleKey?: TranslationKey;
}>;

const kimiBaseUrlPresets: BaseUrlPreset[] = [
  {
    labelKey: "settings.connection.baseUrlPresetInternational",
    url: "https://api.moonshot.ai/v1",
  },
  {
    labelKey: "settings.connection.baseUrlPresetMainlandChina",
    url: "https://api.moonshot.cn/v1",
  },
];

export function SettingsDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useI18n();
  const [settings, setSettings] = useState<ProviderSettingsResponse | null>(
    null,
  );
  const [section, setSection] = useState<SettingsSectionId>("providers");
  const [selectedProviderId, setSelectedProviderId] = useState("openai");
  const [connectionPreset, setConnectionPreset] =
    useState<ProviderPreset | null>(null);
  const [openAiAuthMode, setOpenAiAuthMode] =
    useState<OpenAiAuthMode>("api_key");
  const [baseUrl, setBaseUrl] = useState(defaultBaseUrl);
  const [model, setModel] = useState(defaultModel);
  const [apiKey, setApiKey] = useState("");
  const [clearApiKey, setClearApiKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [oauthBusy, setOauthBusy] = useState(false);
  const [oauthDevice, setOauthDevice] = useState<OAuthDeviceState | null>(null);
  const [testing, setTesting] = useState(false);
  const [discoveringModels, setDiscoveringModels] = useState(false);
  const [connectionResult, setConnectionResult] =
    useState<ScopedConnectionResult | null>(null);
  const [modelDiscoveryResult, setModelDiscoveryResult] =
    useState<ScopedModelDiscoveryResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      return;
    }
    let cancelled = false;
    setError(null);
    setConnectionPreset(null);
    setSection("providers");
    void exagentClient
      .getProviderSettings()
      .then((nextSettings) => {
        if (cancelled) {
          return;
        }
        setSettings(nextSettings);
        setSelectedProviderId(nextSettings.active_provider_id);
        setBaseUrl(nextSettings.config.base_url);
        setModel(nextSettings.config.model);
        setApiKey("");
        setClearApiKey(false);
        setConnectionResult(null);
        setModelDiscoveryResult(null);
        setOauthDevice(null);
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          setError(errorMessage(reason));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  const selectedProvider = useMemo(
    () =>
      settings?.providers.find(
        (provider) => provider.id === selectedProviderId,
      ) ?? null,
    [selectedProviderId, settings?.providers],
  );
  const selectedProviderConfig = useMemo(
    () =>
      providerConfigForSettings(settings, selectedProviderId) ??
      (selectedProvider ? defaultProviderConfig(selectedProvider) : null),
    [selectedProvider, selectedProviderId, settings],
  );
  const connectionProvider = useMemo(() => {
    if (!connectionPreset) {
      return null;
    }
    const provider = settings?.providers.find(
      (item) => item.id === connectionPreset.providerId,
    );
    return provider ? { provider, preset: connectionPreset } : null;
  }, [connectionPreset, settings?.providers]);
  const hasSavedApiKey =
    selectedProviderConfig?.credential_source === "keychain" &&
    selectedProviderConfig?.credential_kind === "api_key";
  const canUseSavedApiKey =
    hasSavedApiKey && apiKey.trim() === "" && !clearApiKey;
  const currentConnectionKey = selectedProvider
    ? connectionTestKey(
        selectedProvider.id,
        baseUrl,
        model,
        apiKey,
        canUseSavedApiKey,
      )
    : "";
  const currentModelDiscoveryKey = selectedProvider
    ? modelDiscoveryKey(selectedProvider.id, baseUrl, apiKey, canUseSavedApiKey)
    : "";
  const currentFormMatchesSavedSettings =
    selectedProvider?.id === settings?.active_provider_id &&
    baseUrl === settings?.config.base_url &&
    model === settings?.config.model &&
    apiKey.trim() === "" &&
    !clearApiKey;
  const displayedConnectionResult =
    (connectionResult?.requestKey === currentConnectionKey
      ? connectionResult.result
      : null) ??
    (currentFormMatchesSavedSettings
      ? (settings?.last_connection ?? null)
      : null);
  const displayedModelDiscoveryResult =
    modelDiscoveryResult?.requestKey === currentModelDiscoveryKey
      ? modelDiscoveryResult.result
      : null;

  function updateBaseUrl(value: string) {
    setBaseUrl(value);
    clearTransientResults();
  }

  function updateModel(value: string) {
    setModel(value);
    clearTransientResults();
  }

  function selectDiscoveredModel(value: string) {
    setModel(value);
    setConnectionResult(null);
  }

  function updateApiKey(value: string) {
    setApiKey(value);
    clearTransientResults();
  }

  function updateClearApiKey(value: boolean) {
    setClearApiKey(value);
    clearTransientResults();
  }

  function clearTransientResults() {
    setConnectionResult(null);
    setModelDiscoveryResult(null);
  }

  function selectSection(nextSection: SettingsSectionId) {
    setSection(nextSection);
    requestAnimationFrame(() =>
      document.getElementById(`settings-tab-${nextSection}`)?.focus(),
    );
  }

  function handleSectionKeyDown(
    event: KeyboardEvent<HTMLButtonElement>,
    currentSection: SettingsSectionId,
  ) {
    const currentIndex = settingsSections.findIndex(
      (item) => item.id === currentSection,
    );
    if (currentIndex < 0) {
      return;
    }
    const lastIndex = settingsSections.length - 1;
    let nextIndex: number | null = null;

    switch (event.key) {
      case "ArrowDown":
      case "ArrowRight":
        nextIndex = currentIndex === lastIndex ? 0 : currentIndex + 1;
        break;
      case "ArrowUp":
      case "ArrowLeft":
        nextIndex = currentIndex === 0 ? lastIndex : currentIndex - 1;
        break;
      case "Home":
        nextIndex = 0;
        break;
      case "End":
        nextIndex = lastIndex;
        break;
      default:
        return;
    }

    event.preventDefault();
    selectSection(settingsSections[nextIndex].id);
  }

  function configureProvider(preset: ProviderPreset) {
    if (!settings) {
      return;
    }
    const provider = settings.providers.find(
      (item) => item.id === preset.providerId,
    );
    if (!provider) {
      return;
    }
    setSelectedProviderId(provider.id);
    setConnectionPreset(preset);
    setOpenAiAuthMode("api_key");
    const providerConfig = providerConfigForSettings(settings, provider.id);
    setBaseUrl(
      preset.defaultBaseUrl ??
        providerConfig?.base_url ??
        provider.default_base_url,
    );
    setModel(
      preset.defaultModel ?? providerConfig?.model ?? provider.default_model,
    );
    setApiKey("");
    setClearApiKey(false);
    setError(null);
    setOauthDevice(null);
    clearTransientResults();
  }

  async function saveProvider() {
    if (
      !selectedProvider?.supported ||
      (selectedProvider.id === "openai" && openAiAuthMode !== "api_key")
    ) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const discoveredModelsForCurrentForm =
        modelDiscoveryResult?.requestKey === currentModelDiscoveryKey &&
        modelDiscoveryResult.result.status === "success"
          ? modelDiscoveryResult.result.models
          : await discoverModelsForSave();
      const nextSettings = await exagentClient.saveProviderSettings({
        providerId: selectedProvider.id,
        baseUrl,
        model,
        apiKey: apiKey.trim() ? apiKey.trim() : null,
        clearApiKey,
        modelOptions: discoveredModelsForCurrentForm,
      });
      const nextSettingsWithModels =
        mergeDiscoveredModels(
          nextSettings,
          selectedProvider.id,
          discoveredModelsForCurrentForm,
        ) ?? nextSettings;
      setSettings(nextSettingsWithModels);
      setSelectedProviderId(nextSettingsWithModels.active_provider_id);
      setBaseUrl(nextSettingsWithModels.config.base_url);
      setModel(nextSettingsWithModels.config.model);
      setApiKey("");
      setClearApiKey(false);
      setConnectionResult(null);
      applyProviderSettings(nextSettingsWithModels);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  async function testProvider() {
    if (
      !selectedProvider?.supported ||
      (selectedProvider.id === "openai" && openAiAuthMode !== "api_key")
    ) {
      return;
    }
    setTesting(true);
    setError(null);
    setConnectionResult(null);
    const requestKey = currentConnectionKey;
    try {
      const result = await exagentClient.testProviderConnection({
        providerId: selectedProvider.id,
        baseUrl,
        model,
        apiKey: apiKey.trim() ? apiKey.trim() : null,
        useSavedApiKey: canUseSavedApiKey,
      });
      setConnectionResult({ requestKey, result });
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setTesting(false);
    }
  }

  async function discoverModels() {
    if (
      !selectedProvider?.supported ||
      !selectedProvider.supports_model_discovery
    ) {
      return;
    }
    setDiscoveringModels(true);
    setError(null);
    setModelDiscoveryResult(null);
    const requestKey = currentModelDiscoveryKey;
    try {
      const result = await exagentClient.listProviderModels({
        providerId: selectedProvider.id,
        baseUrl,
        apiKey: apiKey.trim() ? apiKey.trim() : null,
        useSavedApiKey: canUseSavedApiKey,
      });
      setModelDiscoveryResult({ requestKey, result });
      if (result.status === "success") {
        const nextSettings = mergeDiscoveredModels(
          settings,
          selectedProvider.id,
          result.models,
        );
        if (nextSettings) {
          setSettings(nextSettings);
          applyProviderSettings(nextSettings);
        }
      }
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setDiscoveringModels(false);
    }
  }

  async function discoverModelsForSave(): Promise<ProviderModelView[]> {
    if (
      !selectedProvider?.supported ||
      !selectedProvider.supports_model_discovery
    ) {
      return [];
    }
    setDiscoveringModels(true);
    const requestKey = currentModelDiscoveryKey;
    try {
      const result = await exagentClient.listProviderModels({
        providerId: selectedProvider.id,
        baseUrl,
        apiKey: apiKey.trim() ? apiKey.trim() : null,
        useSavedApiKey: canUseSavedApiKey,
      });
      setModelDiscoveryResult({ requestKey, result });
      return result.status === "success" ? result.models : [];
    } finally {
      setDiscoveringModels(false);
    }
  }

  async function startOAuthDeviceLogin(
    providerId: "openai" | "github_copilot",
  ) {
    setOauthBusy(true);
    setError(null);
    try {
      const device =
        providerId === "openai"
          ? await exagentClient.startChatGptOAuthDevice()
          : await exagentClient.startGitHubCopilotOAuthDevice();
      setOauthDevice({ providerId, device } as OAuthDeviceState);
      await openOAuthVerificationPage(device.verification_uri);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setOauthBusy(false);
    }
  }

  async function openOAuthVerificationPage(url: string) {
    try {
      await exagentClient.openExternalUrl(url);
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  async function completeOAuthDeviceLogin(
    providerId: "openai" | "github_copilot",
  ) {
    if (!oauthDevice || oauthDevice.providerId !== providerId) {
      return;
    }
    setOauthBusy(true);
    setError(null);
    try {
      const nextSettings =
        providerId === "openai"
          ? await exagentClient.completeChatGptOAuthDevice(
              oauthDevice.device as ChatGptDeviceCode,
            )
          : await exagentClient.completeGitHubCopilotOAuthDevice(
              oauthDevice.device as GitHubCopilotDeviceCode,
            );
      setSettings(nextSettings);
      setSelectedProviderId(nextSettings.active_provider_id);
      setBaseUrl(nextSettings.config.base_url);
      setModel(nextSettings.config.model);
      setApiKey("");
      setClearApiKey(false);
      setOauthDevice(null);
      applyProviderSettings(nextSettings);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setOauthBusy(false);
    }
  }

  if (connectionProvider) {
    return (
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="flex h-[min(720px,calc(100dvh-64px))] w-[calc(100vw-48px)] max-w-[920px] flex-col overflow-hidden p-0 sm:w-[calc(100vw-64px)] md:h-[min(760px,calc(100dvh-96px))]">
          <ProviderConnectionPage
            mark={connectionProvider.preset.mark}
            name={connectionProvider.preset.name}
            provider={connectionProvider.provider}
            onBack={() => {
              setConnectionPreset(null);
              setError(null);
              clearTransientResults();
            }}
            backLabel={t("settings.providers.back")}
          >
            {renderConnectionContent({
              apiKey,
              baseUrl,
              clearApiKey,
              connectionResult: displayedConnectionResult,
              discoveringModels,
              error,
              hasSavedApiKey,
              model,
              modelDiscoveryResult: displayedModelDiscoveryResult,
              openAiAuthMode,
              oauthBusy,
              oauthDevice,
              provider: connectionProvider.provider,
              providerName: connectionProvider.preset.name,
              saving,
              testing,
              onApiKeyChange: updateApiKey,
              onBaseUrlChange: updateBaseUrl,
              onClearApiKeyChange: updateClearApiKey,
              onDiscoverModels: discoverModels,
              onModelChange: updateModel,
              onOpenAiAuthModeChange: setOpenAiAuthMode,
              onStartOAuthDevice: startOAuthDeviceLogin,
              onCompleteOAuthDevice: completeOAuthDeviceLogin,
              onOpenOAuthVerificationPage: openOAuthVerificationPage,
              onSave: saveProvider,
              onSelectDiscoveredModel: selectDiscoveredModel,
              onTestConnection: testProvider,
              t,
            })}
          </ProviderConnectionPage>
        </DialogContent>
      </Dialog>
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[min(720px,calc(100dvh-64px))] w-[calc(100vw-48px)] max-w-[920px] flex-col gap-0 overflow-hidden p-0 sm:w-[calc(100vw-64px)] md:h-[min(760px,calc(100dvh-96px))]">
        <DialogHeader className="shrink-0 border-b border-border px-5 py-4">
          <DialogTitle>{t("settings.title")}</DialogTitle>
          <DialogDescription>{t("settings.description")}</DialogDescription>
        </DialogHeader>

        <div className="grid min-h-0 flex-1 grid-cols-1 grid-rows-[auto_minmax(0,1fr)] overflow-hidden md:grid-cols-[180px_minmax(0,1fr)] md:grid-rows-1">
          <nav
            role="tablist"
            className="shrink-0 border-b border-border p-3 md:min-h-0 md:border-b-0 md:border-r"
            aria-label={t("settings.sections.aria")}
          >
            <div className="grid grid-cols-2 gap-1 md:grid-cols-1">
              {settingsSections.map((item) => {
                const Icon = item.icon;
                const selected = section === item.id;
                return (
                  <button
                    key={item.id}
                    id={`settings-tab-${item.id}`}
                    type="button"
                    role="tab"
                    aria-selected={selected}
                    aria-controls={`settings-panel-${item.id}`}
                    tabIndex={selected ? 0 : -1}
                    className={cn(
                      "type-label-md flex min-h-10 w-full items-center gap-2 rounded-md px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
                      selected
                        ? "bg-surface-2 text-ink"
                        : "text-muted hover:bg-surface-2 hover:text-ink",
                    )}
                    onClick={() => setSection(item.id)}
                    onKeyDown={(event) => handleSectionKeyDown(event, item.id)}
                  >
                    <Icon
                      className={cn(
                        "h-4 w-4",
                        selected ? "text-muted" : "text-subtle",
                      )}
                    />
                    {t(item.labelKey)}
                  </button>
                );
              })}
            </div>
          </nav>

          <div className="min-h-0 min-w-0 overflow-y-auto px-5 py-5 md:px-6 md:py-6">
            <section
              id="settings-panel-general"
              role="tabpanel"
              aria-labelledby="settings-tab-general"
              hidden={section !== "general"}
            >
              <GeneralSettingsPanel />
            </section>
            <section
              id="settings-panel-providers"
              role="tabpanel"
              aria-labelledby="settings-tab-providers"
              hidden={section !== "providers"}
            >
              <ProvidersSettingsPanel
                selectedProviderId={selectedProviderId}
                settings={settings}
                onConfigureProvider={configureProvider}
              />
            </section>
            <section
              id="settings-panel-mcp"
              role="tabpanel"
              aria-labelledby="settings-tab-mcp"
              hidden={section !== "mcp"}
            >
              <McpSettingsPanel />
            </section>
            <section
              id="settings-panel-skills"
              role="tabpanel"
              aria-labelledby="settings-tab-skills"
              hidden={section !== "skills"}
            >
              <SkillsSettingsPanel />
            </section>
            <section
              id="settings-panel-archive"
              role="tabpanel"
              aria-labelledby="settings-tab-archive"
              hidden={section !== "archive"}
            >
              <ArchivedConversationsPanel active={section === "archive"} />
            </section>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function ProvidersSettingsPanel({
  selectedProviderId,
  settings,
  onConfigureProvider,
}: {
  selectedProviderId: string;
  settings: ProviderSettingsResponse | null;
  onConfigureProvider: (preset: ProviderPreset) => void;
}) {
  const { t } = useI18n();
  const presets = useMemo(() => providerPresets(settings), [settings]);

  return (
    <SettingsPanel>
      <SettingsPanelHeader
        title={t("settings.providers.title")}
        description={t("settings.providers.description")}
        action={
          settings?.connected_provider ? (
            <Badge variant="success">{settings.connected_provider.name}</Badge>
          ) : null
        }
      />

      <div className="space-y-3">
        <h3 className="type-title-sm tracking-normal text-subtle">
          {t("settings.providers.popular")}
        </h3>
        <div data-testid="provider-popular-list" className="space-y-1">
          {presets.length > 0 ? (
            presets.map((preset) => {
              const active = preset.providerId === settings?.active_provider_id;
              return (
                <ProviderRow
                  key={preset.key}
                  active={active}
                  preset={preset}
                  selected={preset.providerId === selectedProviderId}
                  supported={Boolean(
                    settings?.providers.find(
                      (provider) => provider.id === preset.providerId,
                    )?.supported,
                  )}
                  t={t}
                  onConnect={() => onConfigureProvider(preset)}
                />
              );
            })
          ) : (
            <div className="type-body-md px-1 py-5 text-muted">
              {t("settings.providers.loading")}
            </div>
          )}
        </div>
      </div>
    </SettingsPanel>
  );
}

function GeneralSettingsPanel() {
  const { t } = useI18n();

  return (
    <SettingsPanel>
      <SettingsPanelHeader
        title={t("settings.general.title")}
        description={t("settings.general.description")}
      />
      <ThemePreferenceSection />
      <LanguagePreferenceSection />
    </SettingsPanel>
  );
}

function ThemePreferenceSection() {
  const { themePreference, setThemePreference } = useThemePreference();
  const { t } = useI18n();
  const options: Array<{
    id: ThemePreference;
    title: string;
    description: string;
    icon: LucideIcon;
  }> = [
    {
      id: "system",
      title: t("settings.theme.system.title"),
      description: t("settings.theme.system.description"),
      icon: Monitor,
    },
    {
      id: "light",
      title: t("settings.theme.light.title"),
      description: t("settings.theme.light.description"),
      icon: Sun,
    },
    {
      id: "dark",
      title: t("settings.theme.dark.title"),
      description: t("settings.theme.dark.description"),
      icon: Moon,
    },
  ];

  return (
    <PreferenceSection
      title={t("settings.theme.title")}
      description={t("settings.theme.description")}
      current={
        options.find((option) => option.id === themePreference)?.title ??
        t("settings.theme.system.title")
      }
      note={t("settings.theme.note")}
    >
      <PreferenceRadioGroup
        ariaLabel={t("settings.theme.current")}
        options={options}
        selectedId={themePreference}
        onSelect={setThemePreference}
      />
    </PreferenceSection>
  );
}

function LanguagePreferenceSection() {
  const { locale, setLocale, t } = useI18n();
  const options: Array<{
    id: Locale;
    title: string;
    description: string;
  }> = [
    {
      id: "en",
      title: t("settings.language.english.title"),
      description: t("settings.language.english.description"),
    },
    {
      id: "zh",
      title: t("settings.language.chinese.title"),
      description: t("settings.language.chinese.description"),
    },
  ];

  return (
    <PreferenceSection
      title={t("settings.language.title")}
      description={t("settings.language.description")}
      current={locale === "zh" ? "中文" : "English"}
      note={t("settings.language.note")}
    >
      <PreferenceRadioGroup
        ariaLabel={t("settings.language.current")}
        options={options}
        selectedId={locale}
        onSelect={setLocale}
      />
    </PreferenceSection>
  );
}

function PreferenceSection({
  children,
  current,
  description,
  note,
  title,
}: {
  children: ReactNode;
  current: string;
  description: string;
  note: string;
  title: string;
}) {
  return (
    <div className="space-y-3">
      <SettingsPanelHeader
        title={title}
        description={description}
        action={<Badge variant="neutral">{current}</Badge>}
      />
      {children}
      <p className="type-body-md text-muted">{note}</p>
    </div>
  );
}

function PreferenceRadioGroup<TId extends string>({
  ariaLabel,
  onSelect,
  options,
  selectedId,
}: {
  ariaLabel: string;
  onSelect: (id: TId) => void;
  options: Array<{
    id: TId;
    title: string;
    description: string;
    icon?: LucideIcon;
  }>;
  selectedId: TId;
}) {
  return (
    <div
      className="rounded-lg border border-border bg-surface-1 p-2"
      role="radiogroup"
      aria-label={ariaLabel}
    >
      {options.map((option) => {
        const selected = selectedId === option.id;
        const Icon = option.icon;
        return (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={selected}
            className={cn(
              "flex min-h-[72px] w-full items-start gap-3 rounded-md px-3 py-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
              selected
                ? "bg-surface-2 text-ink"
                : "text-muted hover:bg-surface-2/70 hover:text-ink",
            )}
            onClick={() => onSelect(option.id)}
          >
            <span
              aria-hidden
              className={cn(
                "mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full border",
                selected
                  ? "border-primary bg-primary text-primary-foreground"
                  : "border-border-strong",
              )}
            >
              {selected ? <Check className="h-3.5 w-3.5" /> : null}
            </span>
            {Icon ? <Icon className="mt-0.5 h-5 w-5 shrink-0 text-subtle" /> : null}
            <span className="min-w-0 flex-1">
              <span className="type-label-md block text-ink">
                {option.title}
              </span>
              <span className="type-body-md mt-1 block text-muted">
                {option.description}
              </span>
            </span>
            {selected ? <CurrentBadge /> : null}
          </button>
        );
      })}
    </div>
  );
}

function CurrentBadge() {
  const { t } = useI18n();
  return <Badge variant="primary">{t("common.current")}</Badge>;
}

function ProviderConnectionPage({
  children,
  mark,
  name,
  provider,
  backLabel,
  onBack,
}: {
  children: ReactNode;
  mark: string;
  name: string;
  provider: ProviderDescriptor;
  backLabel: string;
  onBack: () => void;
}) {
  const { t } = useI18n();

  return (
    <section className="min-h-0 flex-1 overflow-y-auto px-5 py-5 sm:px-7 md:px-8">
      <div
        data-testid="provider-connection-body"
        className="mx-auto w-full max-w-[720px] pb-8"
      >
        <button
          type="button"
          aria-label={backLabel}
          className="flex h-10 w-10 items-center justify-center rounded-md text-muted hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
          onClick={onBack}
        >
          <ArrowLeft className="h-5 w-5" />
        </button>

        <div className="mt-8 flex items-center gap-4">
          <ProviderMark mark={mark} name={name} size="lg" />
          <DialogTitle className="type-title-lg text-ink">
            {t("settings.providers.connect")} {name}
          </DialogTitle>
        </div>

        {children}
      </div>
    </section>
  );
}

function renderConnectionContent({
  apiKey,
  baseUrl,
  clearApiKey,
  connectionResult,
  discoveringModels,
  error,
  hasSavedApiKey,
  model,
  modelDiscoveryResult,
  oauthBusy,
  oauthDevice,
  openAiAuthMode,
  provider,
  providerName,
  saving,
  testing,
  onApiKeyChange,
  onBaseUrlChange,
  onClearApiKeyChange,
  onDiscoverModels,
  onModelChange,
  onCompleteOAuthDevice,
  onOpenAiAuthModeChange,
  onOpenOAuthVerificationPage,
  onStartOAuthDevice,
  onSave,
  onSelectDiscoveredModel,
  onTestConnection,
  t,
}: {
  apiKey: string;
  baseUrl: string;
  clearApiKey: boolean;
  connectionResult: ProviderConnectionTestResponse | null;
  discoveringModels: boolean;
  error: string | null;
  hasSavedApiKey: boolean;
  model: string;
  modelDiscoveryResult: ProviderModelListResponse | null;
  oauthBusy: boolean;
  oauthDevice: OAuthDeviceState | null;
  openAiAuthMode: OpenAiAuthMode;
  provider: ProviderDescriptor;
  providerName: string;
  saving: boolean;
  testing: boolean;
  onApiKeyChange: (value: string) => void;
  onBaseUrlChange: (value: string) => void;
  onClearApiKeyChange: (value: boolean) => void;
  onDiscoverModels: () => void;
  onModelChange: (value: string) => void;
  onCompleteOAuthDevice: (providerId: "openai" | "github_copilot") => void;
  onOpenAiAuthModeChange: (value: OpenAiAuthMode) => void;
  onOpenOAuthVerificationPage: (url: string) => void;
  onStartOAuthDevice: (providerId: "openai" | "github_copilot") => void;
  onSave: () => void;
  onSelectDiscoveredModel: (value: string) => void;
  onTestConnection: () => void;
  t: (key: TranslationKey) => string;
}) {
  if (provider.id === "github_copilot") {
    return (
      <GitHubCopilotConnection
        busy={oauthBusy}
        device={
          oauthDevice?.providerId === "github_copilot"
            ? oauthDevice.device
            : null
        }
        provider={provider}
        t={t}
        onComplete={() => onCompleteOAuthDevice("github_copilot")}
        onOpenVerificationPage={onOpenOAuthVerificationPage}
        onStart={() => onStartOAuthDevice("github_copilot")}
      />
    );
  }

  if (provider.id === "openai") {
    function selectOpenAiAuthMode(nextMode: OpenAiAuthMode) {
      onOpenAiAuthModeChange(nextMode);
      requestAnimationFrame(() =>
        document.getElementById(`openai-auth-mode-${nextMode}`)?.focus(),
      );
    }

    function handleOpenAiAuthModeKeyDown(
      event: KeyboardEvent<HTMLButtonElement>,
      currentMode: OpenAiAuthMode,
    ) {
      const currentIndex = openAiAuthModeOptions.findIndex(
        (item) => item.id === currentMode,
      );
      if (currentIndex < 0) {
        return;
      }
      const lastIndex = openAiAuthModeOptions.length - 1;
      let nextIndex: number | null = null;

      switch (event.key) {
        case "ArrowDown":
        case "ArrowRight":
          nextIndex = currentIndex === lastIndex ? 0 : currentIndex + 1;
          break;
        case "ArrowUp":
        case "ArrowLeft":
          nextIndex = currentIndex === 0 ? lastIndex : currentIndex - 1;
          break;
        case "Home":
          nextIndex = 0;
          break;
        case "End":
          nextIndex = lastIndex;
          break;
        default:
          return;
      }

      event.preventDefault();
      selectOpenAiAuthMode(openAiAuthModeOptions[nextIndex].id);
    }

    return (
      <>
        <DialogDescription className="type-body-lg mt-9 text-muted">
          {t("settings.connection.chooseOpenAi")}
        </DialogDescription>
        <div
          className="mt-4 rounded-lg border border-border-strong bg-surface-1 p-3"
          role="radiogroup"
          aria-label={t("settings.connection.openAiAuthMode")}
        >
          {openAiAuthModeOptions.map((option) => (
            <ConnectionChoice
              key={option.id}
              id={`openai-auth-mode-${option.id}`}
              role="radio"
              title={openAiAuthModeTitle(option, t)}
              selected={openAiAuthMode === option.id}
              tabIndex={openAiAuthMode === option.id ? 0 : -1}
              onKeyDown={(event) =>
                handleOpenAiAuthModeKeyDown(event, option.id)
              }
              onSelect={() => selectOpenAiAuthMode(option.id)}
            />
          ))}
        </div>
        {openAiAuthMode === "api_key" ? (
          <RuntimeProviderForm
            apiKey={apiKey}
            apiKeyLabel={t("settings.connection.openAiApiKey")}
            baseUrl={baseUrl}
            clearApiKey={clearApiKey}
            connectionResult={connectionResult}
            discoveringModels={discoveringModels}
            error={error}
            hasSavedApiKey={hasSavedApiKey}
            model={model}
            modelDiscoveryResult={modelDiscoveryResult}
            provider={provider}
            saving={saving}
            showEndpointFields={false}
            testing={testing}
            onApiKeyChange={onApiKeyChange}
            onBaseUrlChange={onBaseUrlChange}
            onClearApiKeyChange={onClearApiKeyChange}
            onDiscoverModels={onDiscoverModels}
            onModelChange={onModelChange}
            onSave={onSave}
            onSelectDiscoveredModel={onSelectDiscoveredModel}
            onTestConnection={onTestConnection}
            t={t}
          />
        ) : (
          <OAuthDeviceConnection
            busy={oauthBusy}
            device={
              oauthDevice?.providerId === "openai" ? oauthDevice.device : null
            }
            t={t}
            onComplete={() => onCompleteOAuthDevice("openai")}
            onOpenVerificationPage={onOpenOAuthVerificationPage}
            onStart={() => onStartOAuthDevice("openai")}
          />
        )}
      </>
    );
  }

  if (provider.supported) {
    return (
      <>
        <DialogDescription className="type-body-lg mt-9 text-muted">
          {provider.id === "openai_compatible"
            ? t("settings.connection.compatibleDescription")
            : t("settings.connection.apiKeyDescription")}
        </DialogDescription>
        <RuntimeProviderForm
          apiKey={apiKey}
          apiKeyLabel={`${providerName} ${t("settings.connection.apiKey")}`}
          baseUrl={baseUrl}
          clearApiKey={clearApiKey}
          connectionResult={connectionResult}
          discoveringModels={discoveringModels}
          error={error}
          hasSavedApiKey={hasSavedApiKey}
          model={model}
          modelDiscoveryResult={modelDiscoveryResult}
          provider={provider}
          saving={saving}
          showEndpointFields
          testing={testing}
          onApiKeyChange={onApiKeyChange}
          onBaseUrlChange={onBaseUrlChange}
          onClearApiKeyChange={onClearApiKeyChange}
          onDiscoverModels={onDiscoverModels}
          onModelChange={onModelChange}
          onSave={onSave}
          onSelectDiscoveredModel={onSelectDiscoveredModel}
          onTestConnection={onTestConnection}
          t={t}
        />
      </>
    );
  }

  return (
    <PlannedApiKeyConnection
      provider={provider}
      apiKey={apiKey}
      onApiKeyChange={onApiKeyChange}
      t={t}
    />
  );
}

function openAiAuthModeTitle(
  option: (typeof openAiAuthModeOptions)[number],
  t: (key: TranslationKey) => string,
) {
  return "title" in option ? option.title : t(option.titleKey);
}

function RuntimeProviderForm({
  apiKey,
  apiKeyLabel,
  baseUrl,
  clearApiKey,
  connectionResult,
  discoveringModels,
  error,
  hasSavedApiKey,
  model,
  modelDiscoveryResult,
  provider,
  saving,
  showEndpointFields,
  testing,
  onApiKeyChange,
  onBaseUrlChange,
  onClearApiKeyChange,
  onDiscoverModels,
  onModelChange,
  onSave,
  onSelectDiscoveredModel,
  onTestConnection,
  t,
}: {
  apiKey: string;
  apiKeyLabel: string;
  baseUrl: string;
  clearApiKey: boolean;
  connectionResult: ProviderConnectionTestResponse | null;
  discoveringModels: boolean;
  error: string | null;
  hasSavedApiKey: boolean;
  model: string;
  modelDiscoveryResult: ProviderModelListResponse | null;
  provider: ProviderDescriptor;
  saving: boolean;
  showEndpointFields: boolean;
  testing: boolean;
  onApiKeyChange: (value: string) => void;
  onBaseUrlChange: (value: string) => void;
  onClearApiKeyChange: (value: boolean) => void;
  onDiscoverModels: () => void;
  onModelChange: (value: string) => void;
  onSave: () => void;
  onSelectDiscoveredModel: (value: string) => void;
  onTestConnection: () => void;
  t: (key: TranslationKey) => string;
}) {
  const baseUrlPresets = provider.id === "kimi" ? kimiBaseUrlPresets : [];
  const [baseUrlPresetMenuOpen, setBaseUrlPresetMenuOpen] = useState(false);

  function selectBaseUrlPreset(url: string) {
    onBaseUrlChange(url);
    setBaseUrlPresetMenuOpen(false);
  }

  return (
    <form
      className="mt-6 space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        onSave();
      }}
    >
      <label className="type-label-md grid gap-2 text-muted">
        {apiKeyLabel}
        <Input
          type="password"
          value={apiKey}
          placeholder={
            hasSavedApiKey
              ? t("settings.connection.savedInKeychain")
              : t("settings.connection.apiKey")
          }
          className="type-body-lg h-10 px-3"
          onChange={(event) => onApiKeyChange(event.target.value)}
          disabled={clearApiKey}
        />
      </label>

      <div className="grid gap-4">
        {showEndpointFields ? (
          <div className="type-label-md grid gap-2 text-muted">
            <label htmlFor="provider-base-url">
              {t("settings.connection.baseUrl")}
            </label>
            <div className="flex flex-col gap-2 sm:flex-row">
              <Input
                id="provider-base-url"
                className="type-body-lg h-10 min-w-0 flex-1"
                value={baseUrl}
                onChange={(event) => onBaseUrlChange(event.target.value)}
              />
              {baseUrlPresets.length ? (
                <div className="relative">
                  <Button
                    type="button"
                    variant="secondary"
                    className="h-10 w-full sm:w-auto"
                    aria-expanded={baseUrlPresetMenuOpen}
                    aria-haspopup="menu"
                    aria-label={t("settings.connection.baseUrlPresetsAria")}
                    onClick={() =>
                      setBaseUrlPresetMenuOpen((open) => !open)
                    }
                  >
                    {t("settings.connection.baseUrlPresets")}
                    <ChevronDown className="h-4 w-4" />
                  </Button>
                  {baseUrlPresetMenuOpen ? (
                    <div
                      role="menu"
                      className="absolute right-0 top-[calc(100%+6px)] z-50 w-64 rounded-md border border-border bg-surface-2 p-1 text-ink shadow-panel"
                    >
                      {baseUrlPresets.map((preset) => (
                        <button
                          key={preset.url}
                          type="button"
                          role="menuitem"
                          className="type-body-md flex w-full flex-col items-start gap-1 rounded px-2 py-1.5 text-left outline-none transition-colors hover:bg-surface-3 focus:bg-surface-3"
                          onClick={() => selectBaseUrlPreset(preset.url)}
                        >
                          <span>{t(preset.labelKey)}</span>
                          <span className="type-label-sm text-subtle">
                            {preset.url}
                          </span>
                        </button>
                      ))}
                    </div>
                  ) : null}
                </div>
              ) : null}
            </div>
          </div>
        ) : null}
        <div className="type-label-md grid gap-2 text-muted">
          <label htmlFor="provider-model">
            {t("settings.connection.model")}
          </label>
          <div className="flex flex-col gap-2 sm:flex-row">
            <Input
              id="provider-model"
              className="type-body-lg h-10"
              value={model}
              onChange={(event) => onModelChange(event.target.value)}
            />
            {provider.supports_model_discovery ? (
              <Button
                type="button"
                variant="secondary"
                className="h-10 w-full sm:w-auto"
                disabled={discoveringModels}
                onClick={onDiscoverModels}
              >
                <RefreshCw className="h-4 w-4" />
                {discoveringModels
                  ? t("settings.connection.discovering")
                  : t("settings.connection.discoverModels")}
              </Button>
            ) : null}
          </div>
          <ModelDiscoveryResult
            result={modelDiscoveryResult}
            onSelectModel={onSelectDiscoveredModel}
            t={t}
          />
        </div>
      </div>

      {hasSavedApiKey ? (
        <label className="type-body-md flex items-center gap-2 text-muted">
          <input
            type="checkbox"
            checked={clearApiKey}
            onChange={(event) => onClearApiKeyChange(event.target.checked)}
          />
          {t("settings.connection.clearSavedApiKey")}
        </label>
      ) : null}

      {error ? (
        <p className="type-body-md text-danger" role="alert">
          {error}
        </p>
      ) : null}
      {connectionResult ? (
        <p
          role="status"
          aria-live="polite"
          className={cn(
            "type-body-md",
            connectionResult.status === "success"
              ? "text-success"
              : "text-warning",
          )}
        >
          {connectionResult.message}
        </p>
      ) : null}

      <div className="flex flex-col gap-2 pt-1 sm:flex-row">
        <Button type="submit" className="h-9 px-4" disabled={saving}>
          <KeyRound className="h-4 w-4" />
          {saving
            ? t("settings.connection.saving")
            : t("settings.connection.saveProvider")}
        </Button>
        <Button
          type="button"
          variant="secondary"
          className="h-9 px-4"
          disabled={testing}
          onClick={onTestConnection}
        >
          {testing
            ? t("settings.connection.testing")
            : t("settings.connection.testConnection")}
        </Button>
      </div>
    </form>
  );
}

function PlannedApiKeyConnection({
  apiKey,
  provider,
  onApiKeyChange,
  t,
}: {
  apiKey: string;
  provider: ProviderDescriptor;
  onApiKeyChange: (value: string) => void;
  t: (key: TranslationKey) => string;
}) {
  return (
    <>
      <DialogDescription className="type-body-lg mt-9 text-muted">
        {t("settings.connection.plannedProvider")}
      </DialogDescription>
      <div className="mt-6 space-y-4">
        <label className="type-label-md grid gap-2 text-muted">
          {provider.name} {t("settings.connection.apiKey")}
          <Input
            type="password"
            value={apiKey}
            placeholder={t("settings.connection.apiKey")}
            className="type-body-lg h-10 px-3"
            onChange={(event) => onApiKeyChange(event.target.value)}
          />
        </label>
        <Button disabled>{t("settings.connection.comingSoon")}</Button>
        {provider.unsupported_reason ? (
          <p className="type-body-md text-warning">
            {provider.unsupported_reason}
          </p>
        ) : null}
      </div>
    </>
  );
}

function GitHubCopilotConnection({
  busy,
  device,
  provider,
  t,
  onComplete,
  onOpenVerificationPage,
  onStart,
}: {
  busy: boolean;
  device: ChatGptDeviceCode | GitHubCopilotDeviceCode | null;
  provider: ProviderDescriptor;
  t: (key: TranslationKey) => string;
  onComplete: () => void;
  onOpenVerificationPage: (url: string) => void;
  onStart: () => void;
}) {
  return (
    <>
      <DialogDescription className="type-body-lg mt-9 text-muted">
        {t("settings.connection.githubDeployment")}
      </DialogDescription>
      <div className="mt-6 space-y-3">
        <ConnectionChoice
          title="GitHub.com"
          description={t("settings.connection.githubPublic")}
          selected
        />
        <ConnectionChoice
          title="GitHub Enterprise"
          description={t("settings.connection.githubEnterpriseDescription")}
          disabled
        />
        <OAuthDeviceConnection
          busy={busy}
          device={device}
          t={t}
          onComplete={onComplete}
          onOpenVerificationPage={onOpenVerificationPage}
          onStart={onStart}
        />
        {provider.unsupported_reason ? (
          <p className="type-body-md text-warning">
            {provider.unsupported_reason}
          </p>
        ) : null}
      </div>
    </>
  );
}

function OAuthDeviceConnection({
  busy,
  device,
  t,
  onComplete,
  onOpenVerificationPage,
  onStart,
}: {
  busy: boolean;
  device: ChatGptDeviceCode | GitHubCopilotDeviceCode | null;
  t: (key: TranslationKey) => string;
  onComplete: () => void;
  onOpenVerificationPage: (url: string) => void;
  onStart: () => void;
}) {
  return (
    <div className="mt-6 space-y-4">
      {device ? (
        <div className="rounded-lg border border-border bg-surface-1 p-4">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="type-label-md text-muted">
                {t("settings.connection.oauthCode")}
              </p>
              <p className="type-title-md mt-1 tracking-normal text-ink">
                {device.user_code}
              </p>
            </div>
            <a
              className="type-label-md inline-flex h-9 items-center justify-center gap-2 rounded-md border border-border bg-surface-2 px-3 text-ink hover:bg-surface-3"
              href={device.verification_uri}
              onClick={(event) => {
                event.preventDefault();
                onOpenVerificationPage(device.verification_uri);
              }}
              rel="noreferrer"
              target="_blank"
            >
              <ExternalLink className="h-4 w-4" />
              {t("settings.connection.openOAuthPage")}
            </a>
          </div>
        </div>
      ) : null}
      <div className="flex flex-col gap-2 sm:flex-row">
        <Button
          type="button"
          className="h-9 px-4"
          disabled={busy}
          onClick={onStart}
        >
          <KeyRound className="h-4 w-4" />
          {busy && !device
            ? t("settings.connection.startingOAuth")
            : t("settings.connection.startOAuth")}
        </Button>
        <Button
          type="button"
          variant="secondary"
          className="h-9 px-4"
          disabled={busy || !device}
          onClick={onComplete}
        >
          {busy && device
            ? t("settings.connection.completingOAuth")
            : t("settings.connection.completeOAuth")}
        </Button>
      </div>
    </div>
  );
}

function PlannedConnectionNotice({ reason }: { reason: string }) {
  return <p className="type-body-md mt-6 text-warning">{reason}</p>;
}

function ConnectionChoice({
  id,
  title,
  description,
  disabled = false,
  selected = false,
  role,
  tabIndex,
  onKeyDown,
  onSelect,
}: {
  id?: string;
  title: string;
  description?: string;
  disabled?: boolean;
  selected?: boolean;
  role?: "radio";
  tabIndex?: number;
  onKeyDown?: (event: KeyboardEvent<HTMLButtonElement>) => void;
  onSelect?: () => void;
}) {
  return (
    <button
      id={id}
      type="button"
      disabled={disabled}
      role={role}
      aria-checked={role === "radio" ? selected : undefined}
      aria-pressed={role ? undefined : selected}
      tabIndex={tabIndex}
      className="flex min-h-11 w-full items-center gap-3 rounded-md px-2 py-2 text-left transition-colors hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus disabled:cursor-not-allowed disabled:opacity-55 disabled:hover:bg-transparent"
      onKeyDown={onKeyDown}
      onClick={onSelect}
    >
      <span
        className={cn(
          "flex h-4 w-7 items-center justify-center rounded border border-border",
          selected && "border-focus bg-focus/20",
        )}
        aria-hidden="true"
      >
        {selected ? <Check className="h-3 w-3 text-focus" /> : null}
      </span>
      <span className="type-title-md text-ink">{title}</span>
      {description ? (
        <span className="type-body-md text-subtle">{description}</span>
      ) : null}
    </button>
  );
}

function ModelDiscoveryResult({
  result,
  onSelectModel,
  t,
}: {
  result: ProviderModelListResponse | null;
  onSelectModel: (modelId: string) => void;
  t: (key: TranslationKey) => string;
}) {
  if (!result) {
    return null;
  }

  if (result.status !== "success") {
    return (
      <p className="type-body-md text-warning" role="status" aria-live="polite">
        {result.message}
      </p>
    );
  }

  if (result.models.length === 0) {
    return (
      <p className="type-body-md text-subtle" role="status" aria-live="polite">
        {t("settings.connection.noModels")}
      </p>
    );
  }

  return (
    <div className="flex flex-wrap gap-2 pt-1" role="status" aria-live="polite">
      {result.models.map((providerModel) => (
        <Button
          key={providerModel.id}
          type="button"
          variant="secondary"
          size="sm"
          aria-label={`Use ${providerModel.id}`}
          onClick={() => onSelectModel(providerModel.id)}
        >
          {providerModel.display_name}
        </Button>
      ))}
    </div>
  );
}

function mergeDiscoveredModels(
  settings: ProviderSettingsResponse | null,
  providerId: string,
  models: ProviderModelView[],
): ProviderSettingsResponse | null {
  if (!settings) {
    return settings;
  }

  const keepConfiguredModel = (model: ProviderModelView) =>
    settings.config.provider_id === providerId &&
    model.provider_id === providerId &&
    model.id === settings.config.model;
  const retained = settings.model_options.filter(
    (model) => model.provider_id !== providerId || keepConfiguredModel(model),
  );
  const merged = [...retained];
  const seen = new Set(
    merged.map((model) => `${model.provider_id}:${model.id}`),
  );
  models.forEach((model) => {
    const key = `${model.provider_id}:${model.id}`;
    if (!seen.has(key)) {
      merged.push(model);
      seen.add(key);
    }
  });

  return {
    ...settings,
    model_options: merged,
  };
}

function providerConfigForSettings(
  settings: ProviderSettingsResponse | null,
  providerId: string,
) {
  if (!settings) {
    return null;
  }
  return (
    settings.configured_providers.find(
      (config) => config.provider_id === providerId,
    ) ?? (settings.config.provider_id === providerId ? settings.config : null)
  );
}

function defaultProviderConfig(
  provider: ProviderDescriptor,
): ProviderConfigView {
  return {
    provider_id: provider.id,
    base_url: provider.default_base_url,
    model: provider.default_model,
    has_api_key: false,
    credential_source: "none",
    auth_required: provider.auth_mode === "api_key_required",
  };
}

function ProviderRow({
  active,
  preset,
  selected,
  supported,
  t,
  onConnect,
}: {
  active: boolean;
  preset: ProviderPreset;
  selected: boolean;
  supported: boolean;
  t: (key: TranslationKey) => string;
  onConnect: () => void;
}) {
  const description = providerDescription(preset, t);

  return (
    <button
      type="button"
      className={cn(
        "group flex min-h-[68px] w-full items-center gap-4 rounded-md px-1 py-2 text-left transition-colors hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
        selected && "bg-surface-2",
      )}
      aria-label={`${t("settings.providers.configure")} ${preset.name}`}
      onClick={onConnect}
    >
      <ProviderMark mark={preset.mark} name={preset.name} />
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-3">
          <p className="type-title-lg shrink-0 text-ink">{preset.name}</p>
          {description ? (
            <p className="type-body-md truncate text-muted">{description}</p>
          ) : null}
          {preset.recommended ? (
            <Badge variant="neutral">
              {t("settings.providers.recommended")}
            </Badge>
          ) : null}
          {active ? (
            <Badge variant="success">{t("settings.providers.active")}</Badge>
          ) : null}
          {!supported ? (
            <Badge variant="neutral">{t("settings.providers.planned")}</Badge>
          ) : null}
        </div>
      </div>
    </button>
  );
}

function providerDescription(
  preset: ProviderPreset,
  t: (key: TranslationKey) => string,
) {
  switch (preset.key) {
    case "openai":
      return t("settings.providers.descriptions.openai");
    case "openai_compatible":
      return t("settings.providers.descriptions.openaiCompatible");
    case "anthropic":
      return t("settings.providers.descriptions.anthropic");
    case "github_copilot":
      return t("settings.providers.descriptions.githubCopilot");
    case "google":
      return t("settings.providers.descriptions.google");
    case "deepseek":
      return t("settings.providers.descriptions.deepseek");
    case "kimi":
      return t("settings.providers.descriptions.kimi");
    case "glm":
      return t("settings.providers.descriptions.glm");
    default:
      return preset.description;
  }
}

function ProviderMark({
  mark,
  name,
  size = "md",
}: {
  mark?: string;
  name: string;
  size?: "md" | "lg";
}) {
  return (
    <div
      className={cn(
        "flex shrink-0 items-center justify-center rounded-md text-muted",
        size === "lg" ? "type-title-lg h-11 w-11" : "type-title-md h-9 w-9",
      )}
    >
      {mark ?? providerInitials(name)}
    </div>
  );
}

function providerPresets(
  settings: ProviderSettingsResponse | null,
): ProviderPreset[] {
  if (!settings) {
    return [];
  }
  const providerById = new Map(
    settings.providers.map((provider) => [provider.id, provider]),
  );
  const providers = new Set(providerById.keys());
  const presets: ProviderPreset[] = [];
  const providerOrder = [
    "openai",
    "anthropic",
    "google",
    "deepseek",
    "openai_compatible",
    "github_copilot",
    "kimi",
    "glm",
  ];

  providerOrder.forEach((providerId) => {
    const provider = providerById.get(providerId);
    if (!provider) {
      return;
    }
    presets.push(providerPresetFromDescriptor(provider));
  });
  if (providers.has("openai_compatible")) {
    presets.push(
      {
        key: "openrouter",
        providerId: "openai_compatible",
        name: "OpenRouter",
        description: "",
        mark: "OR",
        defaultBaseUrl: "https://openrouter.ai/api/v1",
        defaultModel: "openrouter/auto",
      },
      {
        key: "vercel_ai_gateway",
        providerId: "openai_compatible",
        name: "Vercel AI Gateway",
        description: "",
        mark: "▲",
        defaultBaseUrl: "https://ai-gateway.vercel.sh/v1",
        defaultModel: "openai/gpt-5.5",
      },
    );
  }
  return presets;
}

function providerPresetFromDescriptor(
  provider: ProviderDescriptor,
): ProviderPreset {
  return {
    key: provider.id,
    providerId: provider.id,
    name: provider.name,
    description: provider.description,
    mark: providerMark(provider),
    recommended: provider.recommended,
  };
}

function providerMark(provider: ProviderDescriptor) {
  switch (provider.id) {
    case "openai":
      return "O";
    case "anthropic":
      return "AI";
    case "github_copilot":
      return "GH";
    case "google":
      return "G";
    case "deepseek":
      return "DS";
    case "kimi":
      return "K";
    case "glm":
      return "GL";
    default:
      return providerInitials(provider.name);
  }
}

function providerInitials(name: string) {
  return name
    .split(/\s+/)
    .slice(0, 2)
    .map((part) => part[0])
    .join("")
    .toUpperCase();
}

function errorMessage(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}

function connectionTestKey(
  providerId: string,
  baseUrl: string,
  model: string,
  apiKey: string,
  useSavedApiKey: boolean,
) {
  return [
    providerId,
    baseUrl.trim(),
    model.trim(),
    apiKey.trim(),
    String(useSavedApiKey),
  ].join("\u001f");
}

function modelDiscoveryKey(
  providerId: string,
  baseUrl: string,
  apiKey: string,
  useSavedApiKey: boolean,
) {
  return [
    providerId,
    baseUrl.trim(),
    apiKey.trim(),
    String(useSavedApiKey),
  ].join("\u001f");
}
