import * as React from "react";
import { cn } from "@/lib/utils";

const Textarea = React.forwardRef<HTMLTextAreaElement, React.TextareaHTMLAttributes<HTMLTextAreaElement>>(
  ({ className, ...props }, ref) => (
    <textarea
      ref={ref}
      className={cn(
        "min-h-[88px] w-full resize-none rounded-md border border-border bg-surface-2 px-3 py-2 text-base leading-[1.55] text-ink placeholder:text-subtle outline-none transition-colors focus:border-border-strong focus:ring-2 focus:ring-focus disabled:cursor-not-allowed disabled:opacity-45",
        className
      )}
      {...props}
    />
  )
);
Textarea.displayName = "Textarea";

export { Textarea };
