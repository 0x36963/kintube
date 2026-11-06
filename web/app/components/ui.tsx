import { LoaderCircle } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";

import { APP_NAME } from "~/lib/brand";

export function cx(...classes: (string | false | null | undefined)[]) {
  return classes.filter(Boolean).join(" ");
}

/** Placeholder mark (a framed "ember"), deliberately unlike any existing video brand. */
export function Logo({ withName = true, className }: { withName?: boolean; className?: string }) {
  return (
    <span className={cx("inline-flex items-center gap-2", className)}>
      <svg viewBox="0 0 32 32" className="size-7 shrink-0" aria-hidden="true">
        <rect x="3" y="5" width="26" height="22" rx="7" className="fill-surface-3" />
        <rect x="3" y="5" width="26" height="22" rx="7" className="fill-none stroke-accent" strokeWidth="2.5" />
        <circle cx="16" cy="16" r="4.5" className="fill-accent" />
      </svg>
      {withName && <span className="text-lg font-semibold tracking-tight">{APP_NAME}</span>}
    </span>
  );
}

export function Spinner({ className }: { className?: string }) {
  return <LoaderCircle className={cx("animate-spin text-fg-muted", className ?? "size-6")} aria-label="Loading" />;
}

export function Avatar({ name, className }: { name: string; className?: string }) {
  const initials =
    name
      .split(/[\s._-]+/)
      .filter((part) => /^\p{L}/u.test(part))
      .slice(0, 2)
      .map((part) => part[0]!.toUpperCase())
      .join("") || "?";
  return (
    <span
      className={cx(
        "inline-grid shrink-0 place-items-center rounded-full bg-accent/90 font-semibold text-accent-fg select-none",
        className ?? "size-8 text-sm",
      )}
      aria-hidden="true"
    >
      {initials}
    </span>
  );
}

export function Button({
  variant = "primary",
  className,
  ...props
}: ComponentProps<"button"> & { variant?: "primary" | "secondary" | "ghost" }) {
  return (
    <button
      className={cx(
        "inline-flex h-10 items-center justify-center gap-2 rounded-full px-5 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50",
        variant === "primary" && "bg-accent text-accent-fg hover:bg-accent-hover",
        variant === "secondary" && "bg-surface-2 text-fg hover:bg-surface-3",
        variant === "ghost" && "text-fg-muted hover:bg-surface-2 hover:text-fg",
        className,
      )}
      {...props}
    />
  );
}

export function EmptyState({
  icon: Icon,
  title,
  children,
  badge,
  action,
}: {
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  children?: ReactNode;
  badge?: string;
  action?: ReactNode;
}) {
  return (
    <div className="relative z-10 mx-auto flex max-w-md flex-col items-center px-6 py-16 text-center">
      <div className="mb-5 grid size-16 place-items-center rounded-2xl bg-surface-2 ring-1 ring-line">
        <Icon className="size-7 text-accent" />
      </div>
      <h1 className="text-xl font-semibold">{title}</h1>
      {badge && (
        <span className="mt-2 rounded-full bg-surface-2 px-2.5 py-0.5 text-xs font-medium text-fg-muted ring-1 ring-line">
          {badge}
        </span>
      )}
      {children && <p className="mt-3 text-sm leading-relaxed text-fg-muted">{children}</p>}
      {action && <div className="mt-6">{action}</div>}
    </div>
  );
}

/** Faded skeleton of the video grid, to show the shape of a page before it has content. */
export function GhostGrid({ count = 8 }: { count?: number }) {
  return (
    <div
      aria-hidden="true"
      className="pointer-events-none grid grid-cols-1 gap-x-4 gap-y-8 opacity-40 [mask-image:linear-gradient(to_bottom,black,transparent)] sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4"
    >
      {Array.from({ length: count }, (_, i) => (
        <div key={i}>
          <div className="skeleton aspect-video rounded-xl" />
          <div className="mt-3 flex gap-3">
            <div className="skeleton size-9 shrink-0 rounded-full" />
            <div className="flex-1 space-y-2 pt-1">
              <div className="skeleton h-3.5 w-11/12 rounded" />
              <div className="skeleton h-3 w-2/3 rounded" />
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}
