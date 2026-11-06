import { LogOut, Menu, Search, Settings, Upload } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { Form, Link, useNavigate, useSearchParams } from "react-router";

import { signOut, useUser } from "~/lib/session";

import { Avatar, Logo, cx } from "./ui";

export function TopBar({ onToggleSidebar }: { onToggleSidebar: () => void }) {
  const [params] = useSearchParams();
  return (
    <header className="sticky top-0 z-40 flex h-14 items-center gap-2 bg-bg/95 px-2 backdrop-blur sm:gap-4 sm:px-4">
      <div className="flex shrink-0 items-center gap-1 sm:gap-3">
        <IconButton label="Toggle sidebar" onClick={onToggleSidebar} className="hidden md:grid">
          <Menu className="size-5" />
        </IconButton>
        <Link to="/" className="rounded-lg px-1">
          <Logo />
        </Link>
      </div>

      <Form
        action="/search"
        role="search"
        className="mx-auto hidden h-10 w-full max-w-xl items-center rounded-full bg-surface ring-1 ring-line focus-within:ring-accent sm:flex"
      >
        <Search className="ml-4 size-4 shrink-0 text-fg-subtle" aria-hidden="true" />
        <input
          name="q"
          type="search"
          defaultValue={params.get("q") ?? ""}
          placeholder="Search your library"
          aria-label="Search your library"
          className="h-full min-w-0 flex-1 bg-transparent px-3 text-sm outline-none placeholder:text-fg-subtle"
        />
      </Form>

      <div className="ml-auto flex shrink-0 items-center gap-1 sm:ml-0 sm:gap-2">
        <Link to="/search" className="sm:hidden" aria-label="Search">
          <span className="grid size-10 place-items-center rounded-full hover:bg-surface-2">
            <Search className="size-5" />
          </span>
        </Link>
        <button
          type="button"
          disabled
          title="Uploading arrives in the next milestone"
          className="hidden h-9 items-center gap-2 rounded-full bg-surface-2 px-4 text-sm font-medium text-fg-muted sm:inline-flex disabled:cursor-not-allowed"
        >
          <Upload className="size-4" />
          Upload
        </button>
        <UserMenu />
      </div>
    </header>
  );
}

function UserMenu() {
  const user = useUser();
  const navigate = useNavigate();

  async function handleSignOut() {
    try {
      await signOut();
    } finally {
      navigate("/login", { replace: true });
    }
  }

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger className="rounded-full p-1 hover:bg-surface-2" aria-label="Account menu">
        <Avatar name={user.display_name} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={6}
          className="z-50 w-72 overflow-hidden rounded-xl bg-surface-2 py-2 shadow-2xl shadow-black/50 ring-1 ring-line"
        >
          <div className="flex items-center gap-3 border-b border-line px-4 pt-2 pb-4">
            <Avatar name={user.display_name} className="size-10" />
            <div className="min-w-0">
              <p className="truncate font-medium">{user.display_name}</p>
              <p className="truncate text-sm text-fg-muted">{user.email}</p>
            </div>
          </div>
          <div className="pt-2">
            <MenuItem onSelect={() => navigate("/settings")}>
              <Settings className="size-5 text-fg-muted" />
              Settings
            </MenuItem>
            <MenuItem onSelect={handleSignOut}>
              <LogOut className="size-5 text-fg-muted" />
              Sign out
            </MenuItem>
          </div>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function MenuItem({ children, onSelect }: { children: React.ReactNode; onSelect: () => void }) {
  return (
    <DropdownMenu.Item
      onSelect={onSelect}
      className="flex h-10 cursor-pointer items-center gap-4 px-4 text-sm outline-none data-highlighted:bg-surface-3"
    >
      {children}
    </DropdownMenu.Item>
  );
}

function IconButton({
  label,
  className,
  ...props
}: React.ComponentProps<"button"> & { label: string }) {
  return (
    <button
      type="button"
      aria-label={label}
      className={cx("size-10 place-items-center rounded-full hover:bg-surface-2", className ?? "grid")}
      {...props}
    />
  );
}
