import * as React from "react";
import { cn } from "@/lib/utils";

const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, type, ...props }, ref) => (
    <input
      ref={ref}
      type={type}
      className={cn(
        "type-body-md h-8 w-full rounded-md border border-border bg-surface-2 px-2.5 text-ink shadow-[inset_0_1px_0_rgb(255_255_255_/_0.030)] placeholder:text-subtle outline-none transition-[background-color,border-color,box-shadow] duration-150 focus:border-primary focus:ring-2 focus:ring-focus disabled:cursor-not-allowed disabled:opacity-45",
        className
      )}
      {...props}
    />
  )
);
Input.displayName = "Input";

export { Input };
