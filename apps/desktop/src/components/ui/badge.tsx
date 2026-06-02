import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium leading-none",
  {
    variants: {
      variant: {
        neutral: "bg-surface-3 text-muted",
        success: "bg-success/15 text-success",
        info: "bg-info/15 text-info",
        warning: "bg-warning/15 text-warning",
        danger: "bg-danger/15 text-danger",
        primary: "bg-primary-muted text-primary-hover"
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
