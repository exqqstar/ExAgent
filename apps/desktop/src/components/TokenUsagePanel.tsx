import type { ThreadTokenUsage } from "@/types";
import { useI18n, type TranslationKey } from "@/lib/i18n";
import { cn } from "@/lib/utils";

type TokenUsageRowData = {
  label: string;
  value: number;
};

export function TokenUsagePanel({
  usage,
  className
}: {
  usage: ThreadTokenUsage | null | undefined;
  className?: string;
}) {
  const { t } = useI18n();

  if (!usage) {
    return <p className={cn("type-body-md text-muted", className)}>{t("tokenUsage.empty")}</p>;
  }

  const totalRows: TokenUsageRowData[] = [
    { label: t("tokenUsage.threadTotal"), value: usage.total.total_tokens },
    { label: t("tokenUsage.input"), value: usage.total.input_tokens },
    { label: t("tokenUsage.output"), value: usage.total.output_tokens }
  ];

  if (usage.total.reasoning_output_tokens > 0) {
    totalRows.push({ label: t("tokenUsage.reasoning"), value: usage.total.reasoning_output_tokens });
  }

  if (usage.total.cached_input_tokens > 0) {
    totalRows.push({ label: t("tokenUsage.cachedInput"), value: usage.total.cached_input_tokens });
  }

  const lastRows: TokenUsageRowData[] = [
    { label: t("tokenUsage.lastTurn"), value: usage.last.total_tokens },
    { label: t("tokenUsage.lastInput"), value: usage.last.input_tokens },
    { label: t("tokenUsage.lastOutput"), value: usage.last.output_tokens }
  ];

  return (
    <div className={cn("space-y-2", className)}>
      <div className="space-y-1">
        {totalRows.map(({ label, value }) => (
          <TokenUsageRow key={label} label={label} value={value} />
        ))}
      </div>
      <div className="space-y-1 border-t border-border pt-2">
        {lastRows.map(({ label, value }) => (
          <TokenUsageRow key={label} label={label} value={value} />
        ))}
      </div>
    </div>
  );
}

export function tokenUsageSummary(usage: ThreadTokenUsage | null | undefined, t?: (key: TranslationKey) => string) {
  if (!usage) {
    return t ? t("tokenUsage.summary.notReported") : "not reported";
  }
  const count = formatCompactCount(usage.total.total_tokens);
  return t ? t("tokenUsage.summary.tokens").replace("{count}", count) : `${count} tokens`;
}

function TokenUsageRow({ label, value }: { label: string; value: number }) {
  return (
    <div className="type-body-md grid min-w-0 grid-cols-[96px_minmax(0,1fr)] items-start gap-2 py-0.5">
      <span className="min-w-0 truncate text-muted">{label}</span>
      <span title={value.toLocaleString()} className="type-code-sm min-w-0 truncate text-right text-ink">
        {value.toLocaleString()}
      </span>
    </div>
  );
}

function formatCompactCount(value: number) {
  if (value >= 1_000_000) {
    return `${trimFixed(value / 1_000_000)}m`;
  }
  if (value >= 1_000) {
    return `${trimFixed(value / 1_000)}k`;
  }
  return value.toLocaleString();
}

function trimFixed(value: number) {
  return value.toFixed(1).replace(/\.0$/, "");
}
