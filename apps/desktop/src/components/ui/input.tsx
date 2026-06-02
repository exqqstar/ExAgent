import * as React from "react";
import { cn } from "@/lib/utils";

const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, type, ...props }, ref) => (
    <input
      ref={ref}
      type={type}
      className={cn(
        "h-8 w-full rounded-md border border-border bg-surface-2 px-2.5 text-sm text-ink placeholder:text-subtle outline-none transition-colors focus:border-border-strong focus:ring-2 focus:ring-focus disabled:cursor-not-allowed disabled:opacity-45",
        className
      )}
      {...props}
    />
  )
);
Input.displayName = "Input";

export { Input };
