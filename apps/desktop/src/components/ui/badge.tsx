import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "type-badge inline-flex items-center rounded border px-1.5 py-0.5",
  {
    variants: {
      variant: {
        neutral: "border-border bg-surface-3 text-muted",
        success: "border-success/25 bg-success/12 text-success",
        info: "border-info/25 bg-info/12 text-info",
        warning: "border-warning/25 bg-warning/12 text-warning",
        danger: "border-danger/25 bg-danger/12 text-danger",
        primary: "border-primary/30 bg-primary-muted text-primary-hover"
      }
    },
    defaultVariants: {
      variant: "neutral"
    }
  }
);

export interface BadgeProps extends React.HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant, className }))} {...props} />;
}

export { Badge, badgeVariants };
