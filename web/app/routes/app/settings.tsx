import { Check, HardDrive, KeyRound, LogOut, Mail } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router";

import { Avatar, Button } from "~/components/ui";
import { APP_NAME } from "~/lib/brand";
import { signOut, useUser } from "~/lib/session";

export function meta() {
  return [{ title: `Settings · ${APP_NAME}` }];
}

export default function Settings() {
  const user = useUser();
  const navigate = useNavigate();
  const [signingOut, setSigningOut] = useState(false);
  const memberSince = new Date(user.created_at).toLocaleDateString(undefined, { month: "long", year: "numeric" });

  async function handleSignOut() {
    setSigningOut(true);
    try {
      await signOut();
    } finally {
      navigate("/login", { replace: true });
    }
  }

  return (
    <div className="mx-auto max-w-3xl py-6">
      <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>

      <Card title="Account">
        <div className="flex items-center gap-4">
          <Avatar name={user.display_name} className="size-14 text-xl" />
          <div className="min-w-0">
            <p className="truncate text-lg font-medium">{user.display_name}</p>
            <p className="truncate text-sm text-fg-muted">{user.email}</p>
            <p className="mt-0.5 text-xs text-fg-subtle">Member since {memberSince}</p>
          </div>
        </div>
      </Card>

      <Card title="Sign-in methods">
        <ul className="divide-y divide-line">
          <Row icon={Mail} title="Email link or code" detail={user.email} status={<Status ok>Active</Status>} />
          <Row
            icon={KeyRound}
            title="Passkeys"
            detail="Sign in with Face ID, Touch ID or a security key"
            status={<Status>Coming in M2</Status>}
          />
        </ul>
      </Card>

      <Card title="Storage">
        <ul>
          <Row
            icon={HardDrive}
            title="20 GB included"
            detail="Usage appears here once uploads are switched on"
            status={<Status>Coming in M1</Status>}
          />
        </ul>
      </Card>

      <div className="mt-8">
        <Button variant="secondary" onClick={handleSignOut} disabled={signingOut}>
          <LogOut className="size-4" />
          Sign out
        </Button>
      </div>
    </div>
  );
}

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mt-6 rounded-2xl bg-surface p-5 ring-1 ring-line">
      <h2 className="mb-4 text-sm font-semibold text-fg-muted">{title}</h2>
      {children}
    </section>
  );
}

function Row({
  icon: Icon,
  title,
  detail,
  status,
}: {
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  detail: string;
  status: React.ReactNode;
}) {
  return (
    <li className="flex list-none items-center gap-4 py-3 first:pt-0 last:pb-0">
      <Icon className="size-5 shrink-0 text-fg-muted" />
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium">{title}</p>
        <p className="truncate text-sm text-fg-muted">{detail}</p>
      </div>
      {status}
    </li>
  );
}

function Status({ ok, children }: { ok?: boolean; children: React.ReactNode }) {
  return ok ? (
    <span className="inline-flex shrink-0 items-center gap-1 text-xs font-medium text-emerald-400">
      <Check className="size-3.5" />
      {children}
    </span>
  ) : (
    <span className="shrink-0 rounded-full bg-surface-2 px-2.5 py-0.5 text-xs text-fg-muted ring-1 ring-line">
      {children}
    </span>
  );
}
