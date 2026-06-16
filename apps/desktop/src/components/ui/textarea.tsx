import * as React from "react";
import { cn } from "@/lib/utils";

const Textarea = React.forwardRef<HTMLTextAreaElement, React.TextareaHTMLAttributes<HTMLTextAreaElement>>(
  ({ className, ...props }, ref) => (
    <textarea
      ref={ref}
      className={cn(
        "control-field type-body-lg min-h-[88px] w-full resize-none rounded-md border border-border bg-surface-2 px-3 py-2 text-ink shadow-[inset_0_1px_0_rgb(255_255_255_/_0.030)] placeholder:text-subtle outline-none transition-[background-color,border-color,box-shadow] duration-150 disabled:cursor-not-allowed disabled:opacity-45",
        className
      )}
      {...props}
    />
  )
);
Textarea.displayName = "Textarea";

export { Textarea };
