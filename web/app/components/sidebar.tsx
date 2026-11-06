import { Settings } from "lucide-react";
import { NavLink } from "react-router";

import { libraryNav, primaryNav, sharedNav, type NavItem } from "~/lib/nav";

import { cx } from "./ui";

/** Left navigation. `expanded` = full guide; otherwise a compact icon rail. */
export function Sidebar({ expanded }: { expanded: boolean }) {
  return (
    <nav
      aria-label="Main"
      className={cx(
        "sticky top-14 hidden h-[calc(100dvh-3.5rem)] shrink-0 overflow-y-auto overscroll-contain pb-4 md:block",
        expanded ? "w-60 px-3" : "w-[4.5rem] px-1",
      )}
    >
      {expanded ? (
        <>
          <Section items={primaryNav} />
          <Section title="Library" items={libraryNav} />
          <Section items={sharedNav} />
          <Section items={[{ to: "/settings", label: "Settings", icon: Settings }]} />
        </>
      ) : (
        <ul className="space-y-1 pt-1">
          {[...primaryNav, libraryNav[0]!, ...sharedNav].map((item) => (
            <li key={item.to}>
              <RailLink item={item} />
            </li>
          ))}
        </ul>
      )}
    </nav>
  );
}

function Section({ title, items }: { title?: string; items: NavItem[] }) {
  return (
    <div className="border-b border-line py-3 first:pt-1 last:border-b-0">
      {title && <h2 className="px-3 pb-1.5 text-sm font-semibold">{title}</h2>}
      <ul className="space-y-0.5">
        {items.map((item) => (
          <li key={item.to}>
            <GuideLink item={item} />
          </li>
        ))}
      </ul>
    </div>
  );
}

function GuideLink({ item: { to, label, icon: Icon } }: { item: NavItem }) {
  return (
    <NavLink
      to={to}
      end={to === "/"}
      className={({ isActive }) =>
        cx(
          "flex h-10 items-center gap-5 rounded-lg px-3 text-sm transition-colors",
          isActive ? "bg-surface-2 font-medium text-fg" : "text-fg-muted hover:bg-surface hover:text-fg",
        )
      }
    >
      {({ isActive }) => (
        <>
          <Icon className={cx("size-5 shrink-0", isActive && "text-accent")} />
          <span className="truncate">{label}</span>
        </>
      )}
    </NavLink>
  );
}

function RailLink({ item: { to, label, icon: Icon } }: { item: NavItem }) {
  return (
    <NavLink
      to={to}
      end={to === "/"}
      className={({ isActive }) =>
        cx(
          "flex flex-col items-center gap-1.5 rounded-lg px-1 py-3.5 text-[10px] leading-none transition-colors",
          isActive ? "text-fg" : "text-fg-muted hover:bg-surface hover:text-fg",
        )
      }
    >
      {({ isActive }) => (
        <>
          <Icon className={cx("size-5", isActive && "text-accent")} />
          <span className="max-w-full truncate">{label.split(" ")[0]}</span>
        </>
      )}
    </NavLink>
  );
}

/** Bottom tab bar on small screens, where the sidebar is hidden. */
export function MobileNav() {
  const tabs = [...primaryNav.slice(0, 3), sharedNav[0]!, { to: "/settings", label: "You", icon: Settings }];
  return (
    <nav
      aria-label="Main"
      className="fixed inset-x-0 bottom-0 z-30 grid grid-cols-5 border-t border-line bg-bg/95 pb-[env(safe-area-inset-bottom)] backdrop-blur md:hidden"
    >
      {tabs.map(({ to, label, icon: Icon }) => (
        <NavLink
          key={to}
          to={to}
          end={to === "/"}
          className={({ isActive }) =>
            cx(
              "flex flex-col items-center gap-1 py-2 text-[10px] leading-none",
              isActive ? "text-fg" : "text-fg-muted",
            )
          }
        >
          {({ isActive }) => (
            <>
              <Icon className={cx("size-5", isActive && "text-accent")} />
              <span>{label.split(" ")[0]}</span>
            </>
          )}
        </NavLink>
      ))}
    </nav>
  );
}
