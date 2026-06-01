import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { ArrowLeft, Check, Gauge, KeyRound, Plus, RefreshCw, Server, Settings2, Sparkles } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { McpSettingsPanel } from "@/components/settings/McpSettingsPanel";
import { RuntimeSettingsPanel } from "@/components/settings/RuntimeSettingsPanel";
import { SkillsSettingsPanel } from "@/components/settings/SkillsSettingsPanel";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { exagentClient } from "@/api/exagentClient";
import type {
  ProviderConnectionTestResponse,
  ProviderDescriptor,
  ProviderModelListResponse,
  ProviderSettingsResponse
} from "@/types";
import { cn } from "@/lib/utils";

const defaultBaseUrl = "https://api.openai.com/v1";
const defaultModel = "gpt-4.1";

type ScopedModelDiscoveryResult = {
  requestKey: string;
  result: ProviderModelListResponse;
};

type ScopedConnectionResult = {
  requestKey: string;
  result: ProviderConnectionTestResponse;
};

type OpenAiAuthMode = "browser" | "headless" | "api_key";
type SettingsSectionId = "providers" | "runtime" | "mcp" | "skills";

const settingsSections = [
  { id: "providers", label: "Providers", icon: Settings2 },
  { id: "runtime", label: "Runtime", icon: Gauge },
  { id: "mcp", label: "MCP", icon: Server },
  { id: "skills", label: "Skills", icon: Sparkles }
] as const;

export function SettingsDialog({
  open,
  onOpenChange
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [settings, setSettings] = useState<ProviderSettingsResponse | null>(null);
  const [section, setSection] = useState<SettingsSectionId>("providers");
  const [selectedProviderId, setSelectedProviderId] = useState("openai");
  const [connectionProviderId, setConnectionProviderId] = useState<string | null>(null);
  const [openAiAuthMode, setOpenAiAuthMode] = useState<OpenAiAuthMode>("api_key");
  const [baseUrl, setBaseUrl] = useState(defaultBaseUrl);
  const [model, setModel] = useState(defaultModel);
  const [apiKey, setApiKey] = useState("");
  const [clearApiKey, setClearApiKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [discoveringModels, setDiscoveringModels] = useState(false);
  const [connectionResult, setConnectionResult] = useState<ScopedConnectionResult | null>(null);
  const [modelDiscoveryResult, setModelDiscoveryResult] = useState<ScopedModelDiscoveryResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      return;
    }
    let cancelled = false;
    setError(null);
    setConnectionProviderId(null);
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
    () => settings?.providers.find((provider) => provider.id === selectedProviderId) ?? null,
    [selectedProviderId, settings?.providers]
  );
  const connectionProvider = useMemo(
    () => settings?.providers.find((provider) => provider.id === connectionProviderId) ?? null,
    [connectionProviderId, settings?.providers]
  );
  const hasKeychainCredential =
    selectedProvider?.id === settings?.active_provider_id &&
    settings?.config.credential_source === "keychain";
  const canUseSavedApiKey = hasKeychainCredential && !clearApiKey;
  const currentConnectionKey = selectedProvider
    ? connectionTestKey(selectedProvider.id, baseUrl, model, apiKey, canUseSavedApiKey)
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
    (connectionResult?.requestKey === currentConnectionKey ? connectionResult.result : null) ??
    (currentFormMatchesSavedSettings ? settings?.last_connection ?? null : null);
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

  function configureProvider(provider: ProviderDescriptor) {
    if (!settings) {
      return;
    }
    setSelectedProviderId(provider.id);
    setConnectionProviderId(provider.id);
    setOpenAiAuthMode("api_key");
    if (provider.id === settings.active_provider_id) {
      setBaseUrl(settings.config.base_url);
      setModel(settings.config.model);
    } else {
      setBaseUrl(provider.default_base_url);
      setModel(provider.default_model);
    }
    setApiKey("");
    setClearApiKey(false);
    setError(null);
    clearTransientResults();
  }

  async function saveProvider() {
    if (!selectedProvider?.supported || (selectedProvider.id === "openai" && openAiAuthMode !== "api_key")) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const nextSettings = await exagentClient.saveProviderSettings({
        providerId: selectedProvider.id,
        baseUrl,
        model,
        apiKey: apiKey.trim() ? apiKey.trim() : null,
        clearApiKey
      });
      setSettings(nextSettings);
      setSelectedProviderId(nextSettings.active_provider_id);
      setBaseUrl(nextSettings.config.base_url);
      setModel(nextSettings.config.model);
      setApiKey("");
      setClearApiKey(false);
      setConnectionResult(null);
      setModelDiscoveryResult(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  async function testProvider() {
    if (!selectedProvider?.supported || (selectedProvider.id === "openai" && openAiAuthMode !== "api_key")) {
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
        useSavedApiKey: canUseSavedApiKey
      });
      setConnectionResult({ requestKey, result });
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setTesting(false);
    }
  }

  async function discoverModels() {
    if (!selectedProvider?.supported || !selectedProvider.supports_model_discovery) {
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
        useSavedApiKey: canUseSavedApiKey
      });
      setModelDiscoveryResult({ requestKey, result });
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setDiscoveringModels(false);
    }
  }

  if (connectionProvider) {
    return (
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="flex max-h-[calc(100dvh-64px)] w-[calc(100vw-48px)] max-w-[860px] flex-col overflow-hidden p-0 sm:w-[calc(100vw-64px)] md:max-h-[calc(100dvh-96px)]">
          <ProviderConnectionPage
            provider={connectionProvider}
            onBack={() => {
              setConnectionProviderId(null);
              setError(null);
              clearTransientResults();
            }}
          >
            {renderConnectionContent({
              apiKey,
              baseUrl,
              clearApiKey,
              connectionResult: displayedConnectionResult,
              discoveringModels,
              error,
              hasKeychainCredential,
              model,
              modelDiscoveryResult: displayedModelDiscoveryResult,
              openAiAuthMode,
              provider: connectionProvider,
              saving,
              settings,
              testing,
              onApiKeyChange: updateApiKey,
              onBaseUrlChange: updateBaseUrl,
              onClearApiKeyChange: updateClearApiKey,
              onDiscoverModels: discoverModels,
              onModelChange: updateModel,
              onOpenAiAuthModeChange: setOpenAiAuthMode,
              onSave: saveProvider,
              onSelectDiscoveredModel: updateModel,
              onTestConnection: testProvider
            })}
          </ProviderConnectionPage>
        </DialogContent>
      </Dialog>
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[calc(100dvh-64px)] w-[calc(100vw-48px)] max-w-[920px] flex-col gap-0 overflow-hidden p-0 sm:w-[calc(100vw-64px)] md:max-h-[calc(100dvh-96px)]">
        <DialogHeader className="shrink-0 border-b border-border px-5 py-4">
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>Configure ExAgent Desktop runtime behavior.</DialogDescription>
        </DialogHeader>

        <div className="grid min-h-0 flex-1 grid-cols-1 grid-rows-[auto_minmax(0,1fr)] overflow-hidden md:grid-cols-[180px_minmax(0,1fr)] md:grid-rows-1">
          <nav
            role="tablist"
            className="shrink-0 border-b border-border p-3 md:min-h-0 md:border-b-0 md:border-r"
            aria-label="Settings sections"
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
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm font-medium transition-colors",
                      selected ? "bg-surface-2 text-ink" : "text-muted hover:bg-surface-2 hover:text-ink"
                    )}
                    onClick={() => setSection(item.id)}
                  >
                    <Icon className={cn("h-4 w-4", selected ? "text-muted" : "text-subtle")} />
                    {item.label}
                  </button>
                );
              })}
            </div>
          </nav>

          <section
            id={`settings-panel-${section}`}
            role="tabpanel"
            className="min-h-0 min-w-0 overflow-y-auto px-5 py-5 md:px-6 md:py-6"
            aria-labelledby={`settings-tab-${section}`}
          >
            {section === "providers" ? (
              <ProvidersSettingsPanel
                selectedProviderId={selectedProviderId}
                settings={settings}
                onConfigureProvider={configureProvider}
              />
            ) : null}
            {section === "runtime" ? <RuntimeSettingsPanel /> : null}
            {section === "mcp" ? <McpSettingsPanel /> : null}
            {section === "skills" ? <SkillsSettingsPanel /> : null}
          </section>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function ProvidersSettingsPanel({
  selectedProviderId,
  settings,
  onConfigureProvider
}: {
  selectedProviderId: string;
  settings: ProviderSettingsResponse | null;
  onConfigureProvider: (provider: ProviderDescriptor) => void;
}) {
  return (
    <div className="space-y-5 pb-1">
      <div>
        <h2 id="providers-heading" className="text-[22px] font-semibold text-ink">
          Providers
        </h2>
        <p className="mt-1 text-sm text-muted">
          Connect a provider for new ExAgent runtime sessions.
        </p>
      </div>

      <ConnectedProvider settings={settings} />

      <div className="space-y-3">
        <h3 className="text-sm font-semibold uppercase tracking-normal text-subtle">
          Popular Providers
        </h3>
        <div className="rounded-lg border border-border bg-surface-1">
          {settings?.providers.map((provider, index) => (
            <ProviderRow
              key={provider.id}
              provider={provider}
              active={provider.id === settings.active_provider_id}
              selected={provider.id === selectedProviderId}
              separated={index > 0}
              onConnect={() => onConfigureProvider(provider)}
            />
          )) ?? (
            <div className="px-4 py-5 text-sm text-muted">Loading providers...</div>
          )}
        </div>
      </div>
    </div>
  );
}

function ProviderConnectionPage({
  children,
  provider,
  onBack
}: {
  children: ReactNode;
  provider: ProviderDescriptor;
  onBack: () => void;
}) {
  return (
    <section className="min-h-0 flex-1 overflow-y-auto px-5 py-5 sm:px-7 md:px-8">
      <div data-testid="provider-connection-body" className="mx-auto w-full max-w-[720px] pb-8">
        <button
          type="button"
          aria-label="Back to providers"
          className="flex h-8 w-8 items-center justify-center rounded-md text-muted hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
          onClick={onBack}
        >
          <ArrowLeft className="h-5 w-5" />
        </button>

        <div className="mt-8 flex items-center gap-4">
          <ProviderMark name={provider.name} size="lg" />
          <DialogTitle className="text-[24px] font-semibold leading-tight text-ink">
            连接 {provider.name}
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
  hasKeychainCredential,
  model,
  modelDiscoveryResult,
  openAiAuthMode,
  provider,
  saving,
  settings,
  testing,
  onApiKeyChange,
  onBaseUrlChange,
  onClearApiKeyChange,
  onDiscoverModels,
  onModelChange,
  onOpenAiAuthModeChange,
  onSave,
  onSelectDiscoveredModel,
  onTestConnection
}: {
  apiKey: string;
  baseUrl: string;
  clearApiKey: boolean;
  connectionResult: ProviderConnectionTestResponse | null;
  discoveringModels: boolean;
  error: string | null;
  hasKeychainCredential: boolean;
  model: string;
  modelDiscoveryResult: ProviderModelListResponse | null;
  openAiAuthMode: OpenAiAuthMode;
  provider: ProviderDescriptor;
  saving: boolean;
  settings: ProviderSettingsResponse | null;
  testing: boolean;
  onApiKeyChange: (value: string) => void;
  onBaseUrlChange: (value: string) => void;
  onClearApiKeyChange: (value: boolean) => void;
  onDiscoverModels: () => void;
  onModelChange: (value: string) => void;
  onOpenAiAuthModeChange: (value: OpenAiAuthMode) => void;
  onSave: () => void;
  onSelectDiscoveredModel: (value: string) => void;
  onTestConnection: () => void;
}) {
  if (provider.id === "github_copilot") {
    return <GitHubCopilotConnection provider={provider} />;
  }

  if (provider.id === "openai") {
    return (
      <>
        <DialogDescription className="mt-9 text-base leading-7 text-muted">
          选择 OpenAI 的登录方式。
        </DialogDescription>
        <div className="mt-4 rounded-lg border border-border-strong bg-surface-1 p-3">
          <ConnectionChoice
            title="ChatGPT Pro/Plus (browser)"
            selected={openAiAuthMode === "browser"}
            onSelect={() => onOpenAiAuthModeChange("browser")}
          />
          <ConnectionChoice
            title="ChatGPT Pro/Plus (headless)"
            selected={openAiAuthMode === "headless"}
            onSelect={() => onOpenAiAuthModeChange("headless")}
          />
          <ConnectionChoice
            title="API 密钥"
            selected={openAiAuthMode === "api_key"}
            onSelect={() => onOpenAiAuthModeChange("api_key")}
          />
        </div>
        {openAiAuthMode === "api_key" ? (
          <RuntimeProviderForm
            apiKey={apiKey}
            apiKeyLabel="OpenAI API 密钥"
            baseUrl={baseUrl}
            clearApiKey={clearApiKey}
            connectionResult={connectionResult}
            discoveringModels={discoveringModels}
            error={error}
            hasKeychainCredential={hasKeychainCredential}
            model={model}
            modelDiscoveryResult={modelDiscoveryResult}
            provider={provider}
            settings={settings}
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
          />
        ) : (
          <PlannedConnectionNotice reason="ChatGPT account login is planned for a later desktop auth phase." />
        )}
      </>
    );
  }

  if (provider.supported) {
    return (
      <>
        <DialogDescription className="mt-9 text-base leading-7 text-muted">
          {provider.id === "openai_compatible"
            ? "输入你的 OpenAI-compatible endpoint，并可选择提供 API 密钥。"
            : `输入你的 ${provider.name} API 密钥以连接账户。`}
        </DialogDescription>
        <RuntimeProviderForm
          apiKey={apiKey}
          apiKeyLabel={`${provider.name} API 密钥`}
          baseUrl={baseUrl}
          clearApiKey={clearApiKey}
          connectionResult={connectionResult}
          discoveringModels={discoveringModels}
          error={error}
          hasKeychainCredential={hasKeychainCredential}
          model={model}
          modelDiscoveryResult={modelDiscoveryResult}
          provider={provider}
          settings={settings}
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
        />
      </>
    );
  }

  return <PlannedApiKeyConnection provider={provider} apiKey={apiKey} onApiKeyChange={onApiKeyChange} />;
}

function RuntimeProviderForm({
  apiKey,
  apiKeyLabel,
  baseUrl,
  clearApiKey,
  connectionResult,
  discoveringModels,
  error,
  hasKeychainCredential,
  model,
  modelDiscoveryResult,
  provider,
  settings,
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
  onTestConnection
}: {
  apiKey: string;
  apiKeyLabel: string;
  baseUrl: string;
  clearApiKey: boolean;
  connectionResult: ProviderConnectionTestResponse | null;
  discoveringModels: boolean;
  error: string | null;
  hasKeychainCredential: boolean;
  model: string;
  modelDiscoveryResult: ProviderModelListResponse | null;
  provider: ProviderDescriptor;
  settings: ProviderSettingsResponse | null;
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
}) {
  return (
    <form
      className="mt-6 space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        onSave();
      }}
    >
      <label className="grid gap-2 text-sm font-semibold text-muted">
        {apiKeyLabel}
        <Input
          type="password"
          value={apiKey}
          placeholder={hasKeychainCredential ? "Saved in Keychain" : "API 密钥"}
          className="h-10 px-3 text-base"
          onChange={(event) => onApiKeyChange(event.target.value)}
          disabled={clearApiKey}
        />
      </label>

      {showEndpointFields ? (
        <div className="grid gap-4">
          <label className="grid gap-2 text-sm font-medium text-muted">
            Base URL
            <Input
              className="h-10 text-base"
              value={baseUrl}
              onChange={(event) => onBaseUrlChange(event.target.value)}
            />
          </label>
          <div className="grid gap-2 text-sm font-medium text-muted">
            <label htmlFor="provider-model">Model</label>
            <div className="flex flex-col gap-2 sm:flex-row">
              <Input
                id="provider-model"
                className="h-10 text-base"
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
                  {discoveringModels ? "Discovering" : "Discover models"}
                </Button>
              ) : null}
            </div>
            <ModelDiscoveryResult result={modelDiscoveryResult} onSelectModel={onSelectDiscoveredModel} />
          </div>
        </div>
      ) : null}

      {provider.id === settings?.active_provider_id && hasKeychainCredential ? (
        <label className="flex items-center gap-2 text-sm text-muted">
          <input
            type="checkbox"
            checked={clearApiKey}
            onChange={(event) => onClearApiKeyChange(event.target.checked)}
          />
          Clear saved API key
        </label>
      ) : null}

      {error ? (
        <p className="text-sm text-danger" role="alert">
          {error}
        </p>
      ) : null}
      {connectionResult ? (
        <p
          role="status"
          aria-live="polite"
          className={cn(
            "text-sm",
            connectionResult.status === "success" ? "text-success" : "text-warning"
          )}
        >
          {connectionResult.message}
        </p>
      ) : null}

      <div className="flex flex-col gap-2 pt-1 sm:flex-row">
        <Button type="submit" className="h-9 px-4" disabled={saving}>
          <KeyRound className="h-4 w-4" />
          {saving ? "Saving" : "提交"}
        </Button>
        <Button
          type="button"
          variant="secondary"
          className="h-9 px-4"
          disabled={testing}
          onClick={onTestConnection}
        >
          {testing ? "Testing" : "Test connection"}
        </Button>
      </div>
    </form>
  );
}

function PlannedApiKeyConnection({
  apiKey,
  provider,
  onApiKeyChange
}: {
  apiKey: string;
  provider: ProviderDescriptor;
  onApiKeyChange: (value: string) => void;
}) {
  return (
    <>
      <DialogDescription className="mt-9 text-base leading-7 text-muted">
        输入你的 {provider.name} API 密钥以连接账户，并在 ExAgent 中使用 {provider.name} 模型。
      </DialogDescription>
      <div className="mt-6 space-y-4">
        <label className="grid gap-2 text-sm font-semibold text-muted">
          {provider.name} API 密钥
          <Input
            type="password"
            value={apiKey}
            placeholder="API 密钥"
            className="h-10 px-3 text-base"
            onChange={(event) => onApiKeyChange(event.target.value)}
          />
        </label>
        <Button disabled>即将支持</Button>
        {provider.unsupported_reason ? <p className="text-sm text-warning">{provider.unsupported_reason}</p> : null}
      </div>
    </>
  );
}

function GitHubCopilotConnection({ provider }: { provider: ProviderDescriptor }) {
  return (
    <>
      <DialogDescription className="mt-9 text-base leading-7 text-muted">
        Select GitHub deployment type
      </DialogDescription>
      <div className="mt-6 space-y-3">
        <ConnectionChoice title="GitHub.com" description="Public" disabled />
        <ConnectionChoice title="GitHub Enterprise" description="Data residency or self-hosted" disabled />
        {provider.unsupported_reason ? <p className="text-sm text-warning">{provider.unsupported_reason}</p> : null}
      </div>
    </>
  );
}

function PlannedConnectionNotice({ reason }: { reason: string }) {
  return <p className="mt-6 text-sm text-warning">{reason}</p>;
}

function ConnectionChoice({
  title,
  description,
  disabled = false,
  selected = false,
  onSelect
}: {
  title: string;
  description?: string;
  disabled?: boolean;
  selected?: boolean;
  onSelect?: () => void;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      aria-pressed={selected}
      className="flex min-h-11 w-full items-center gap-3 rounded-md px-2 py-2 text-left transition-colors hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-55 disabled:hover:bg-transparent"
      onClick={onSelect}
    >
      <span
        className={cn(
          "flex h-4 w-7 items-center justify-center rounded border border-border",
          selected && "border-focus bg-focus/20"
        )}
        aria-hidden="true"
      >
        {selected ? <Check className="h-3 w-3 text-focus" /> : null}
      </span>
      <span className="text-base font-semibold text-ink">{title}</span>
      {description ? <span className="text-base text-subtle">{description}</span> : null}
    </button>
  );
}

function ConnectedProvider({ settings }: { settings: ProviderSettingsResponse | null }) {
  if (!settings?.connected_provider) {
    return (
      <div className="rounded-lg border border-border bg-surface-1 px-4 py-5 text-base font-medium text-subtle sm:text-lg">
        No connected provider
      </div>
    );
  }

  return (
    <div className="rounded-lg border border-border bg-surface-1 px-4 py-4">
      <div className="flex items-center gap-3">
        <ProviderMark name={settings.connected_provider.name} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <p className="truncate text-base font-semibold text-ink">{settings.connected_provider.name}</p>
            <Badge variant="success">Connected</Badge>
          </div>
          <p className="mt-1 truncate text-sm text-muted">
            {settings.connected_provider.model} · {settings.connected_provider.base_url}
          </p>
        </div>
      </div>
    </div>
  );
}

function ModelDiscoveryResult({
  result,
  onSelectModel
}: {
  result: ProviderModelListResponse | null;
  onSelectModel: (modelId: string) => void;
}) {
  if (!result) {
    return null;
  }

  if (result.status !== "success") {
    return (
      <p className="text-sm text-warning" role="status" aria-live="polite">
        {result.message}
      </p>
    );
  }

  if (result.models.length === 0) {
    return (
      <p className="text-sm text-subtle" role="status" aria-live="polite">
        No models returned.
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

function ProviderRow({
  provider,
  active,
  selected,
  separated,
  onConnect
}: {
  provider: ProviderDescriptor;
  active: boolean;
  selected: boolean;
  separated: boolean;
  onConnect: () => void;
}) {
  return (
    <div>
      {separated ? <Separator /> : null}
      <div className={cn("flex flex-col gap-3 px-4 py-4 sm:flex-row sm:items-center sm:gap-4", selected && "bg-surface-2")}>
        <ProviderMark name={provider.name} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <p className="truncate text-base font-semibold text-ink">{provider.name}</p>
            {provider.recommended ? <Badge variant="neutral">Recommended</Badge> : null}
            {active ? <Badge variant="success">Active</Badge> : null}
            {!provider.supported ? <Badge variant="neutral">Coming soon</Badge> : null}
          </div>
          <p className="mt-1 truncate text-sm text-muted">{provider.description}</p>
        </div>
        <Button
          type="button"
          variant="secondary"
          className="w-full justify-center sm:w-auto sm:shrink-0"
          aria-label={`Configure ${provider.name}`}
          onClick={onConnect}
        >
          {active ? <Check className="h-4 w-4" /> : <Plus className="h-4 w-4" />}
          Configure
        </Button>
      </div>
    </div>
  );
}

function ProviderMark({
  name,
  size = "md"
}: {
  name: string;
  size?: "md" | "lg";
}) {
  return (
    <div
      className={cn(
        "flex shrink-0 items-center justify-center rounded-md bg-surface-2 font-semibold text-ink",
        size === "lg" ? "h-11 w-11 text-lg" : "h-9 w-9 text-sm"
      )}
    >
      {providerInitials(name)}
    </div>
  );
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
  useSavedApiKey: boolean
) {
  return [
    providerId,
    baseUrl.trim(),
    model.trim(),
    apiKey.trim(),
    String(useSavedApiKey)
  ].join("\u001f");
}

function modelDiscoveryKey(
  providerId: string,
  baseUrl: string,
  apiKey: string,
  useSavedApiKey: boolean
) {
  return [
    providerId,
    baseUrl.trim(),
    apiKey.trim(),
    String(useSavedApiKey)
  ].join("\u001f");
}
