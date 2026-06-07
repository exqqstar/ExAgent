import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export function SettingsPanel({
  children,
  className
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={cn("space-y-5 pb-1", className)}>{children}</div>;
}

export function SettingsPanelHeader({
  action,
  description,
  title
}: {
  action?: ReactNode;
  description: string;
  title: string;
}) {
  return (
    <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
      <div>
        <h2 className="type-title-lg text-ink">{title}</h2>
        <p className="type-body-md mt-1 text-muted">{description}</p>
      </div>
      {action ? <div className="shrink-0">{action}</div> : null}
    </div>
  );
}

export function SettingsPanelCard({
  children,
  className
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={cn("rounded-lg border border-border bg-surface-1", className)}>{children}</div>;
}
