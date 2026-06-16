import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "type-label-md inline-flex h-8 shrink-0 items-center justify-center gap-2 rounded-md border border-transparent px-3 transition-[background-color,border-color,color,box-shadow,filter] duration-150 ease-out focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus disabled:pointer-events-none disabled:opacity-45",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow-[inset_0_1px_0_rgb(255_255_255_/_0.22)] hover:bg-primary-hover active:brightness-[0.98] active:shadow-[inset_0_1px_2px_rgb(0_0_0_/_0.14)]",
        secondary:
          "border-border bg-surface-2 text-ink shadow-[inset_0_1px_0_rgb(255_255_255_/_0.035)] hover:border-border-strong hover:bg-surface-3 active:border-border-strong active:bg-surface-3/80 active:shadow-[inset_0_1px_1px_rgb(0_0_0_/_0.06)]",
        ghost: "text-muted hover:bg-surface-2 hover:text-ink active:bg-surface-2/70 active:text-ink",
        danger: "bg-danger text-white hover:brightness-110 active:brightness-95 active:shadow-[inset_0_1px_2px_rgb(0_0_0_/_0.16)]",
        outline:
          "border-border bg-transparent text-ink hover:border-border-strong hover:bg-surface-2 active:border-border-strong active:bg-surface-2/75"
      },
      size: {
        default: "h-8 px-3",
        sm: "type-label-sm h-7 px-2",
        icon: "h-8 w-8 px-0"
      }
    },
    defaultVariants: {
      variant: "default",
      size: "default"
    }
  }
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return <Comp className={cn(buttonVariants({ variant, size, className }))} ref={ref} {...props} />;
  }
);
Button.displayName = "Button";

export { Button, buttonVariants };
