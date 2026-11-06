import { useMutation } from "@tanstack/react-query";
import { ArrowLeft, Mail } from "lucide-react";
import { useState } from "react";
import { redirect, useNavigate, useSearchParams } from "react-router";

import { Button, Logo, Spinner } from "~/components/ui";
import { ApiError, api, errorMessage, unwrap } from "~/lib/api/client";
import { APP_NAME } from "~/lib/brand";
import { meQuery, queryClient, safeNextPath, setSignedIn } from "~/lib/session";

import type { Route } from "./+types/login";

export async function clientLoader({ request }: Route.ClientLoaderArgs) {
  const me = await queryClient.ensureQueryData(meQuery);
  if (me) throw redirect(safeNextPath(new URL(request.url).searchParams.get("next")));
  return null;
}

export function meta() {
  return [{ title: `Sign in · ${APP_NAME}` }];
}

type Step = { kind: "email" } | { kind: "code"; email: string; loginId: string };

export default function Login() {
  const [step, setStep] = useState<Step>({ kind: "email" });

  return (
    <main className="relative grid min-h-dvh place-items-center overflow-hidden px-4 py-12">
      <div
        aria-hidden="true"
        className="pointer-events-none absolute -top-40 left-1/2 h-[480px] w-[760px] -translate-x-1/2 rounded-full bg-accent/15 blur-3xl"
      />
      <div className="relative w-full max-w-sm">
        <div className="mb-8 flex justify-center">
          <Logo className="scale-125" />
        </div>
        <div className="rounded-2xl bg-surface/80 p-7 shadow-2xl shadow-black/40 ring-1 ring-line backdrop-blur">
          {step.kind === "email" ? (
            <EmailStep onSent={(email, loginId) => setStep({ kind: "code", email, loginId })} />
          ) : (
            <CodeStep
              email={step.email}
              loginId={step.loginId}
              onResent={(loginId) => setStep({ ...step, loginId })}
              onBack={() => setStep({ kind: "email" })}
            />
          )}
        </div>
        <p className="mt-6 text-center text-xs text-fg-subtle">
          Your videos and photos are private unless you choose to share them.
        </p>
      </div>
    </main>
  );
}

async function startLogin(email: string) {
  return unwrap(await api.POST("/api/auth/email/start", { body: { email } }));
}

function EmailStep({ onSent }: { onSent: (email: string, loginId: string) => void }) {
  const [email, setEmail] = useState("");
  const start = useMutation({
    mutationFn: startLogin,
    onSuccess: (res) => onSent(email.trim(), res.login_id),
  });

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        start.mutate(email);
      }}
    >
      <h1 className="text-xl font-semibold">Sign in or create an account</h1>
      <p className="mt-1.5 text-sm text-fg-muted">We'll email you a sign-in link and code. No password needed.</p>

      <label htmlFor="email" className="mt-6 block text-sm font-medium">
        Email
      </label>
      <input
        id="email"
        type="email"
        required
        autoFocus
        autoComplete="email"
        value={email}
        onChange={(e) => setEmail(e.target.value)}
        placeholder="you@example.com"
        className="mt-2 h-11 w-full rounded-lg bg-surface-2 px-3.5 text-sm ring-1 ring-line outline-none placeholder:text-fg-subtle focus:ring-2 focus:ring-accent"
      />
      {start.isError && <ErrorText>{errorMessage(start.error)}</ErrorText>}

      <Button type="submit" className="mt-5 w-full" disabled={start.isPending}>
        {start.isPending ? <Spinner className="size-4 text-accent-fg" /> : <Mail className="size-4" />}
        Continue with email
      </Button>
    </form>
  );
}

function CodeStep({
  email,
  loginId,
  onResent,
  onBack,
}: {
  email: string;
  loginId: string;
  onResent: (loginId: string) => void;
  onBack: () => void;
}) {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [code, setCode] = useState("");
  const [resentNotice, setResentNotice] = useState(false);

  const verify = useMutation({
    mutationFn: async (code: string) =>
      unwrap(await api.POST("/api/auth/email/verify-code", { body: { login_id: loginId, code } })),
    onSuccess: (me) => {
      setSignedIn(me);
      navigate(safeNextPath(params.get("next")), { replace: true });
    },
    onError: () => setCode(""),
  });

  const resend = useMutation({
    mutationFn: () => startLogin(email),
    onSuccess: (res) => {
      verify.reset();
      setResentNotice(true);
      onResent(res.login_id);
    },
  });

  const expired = verify.error instanceof ApiError && verify.error.code === "login_invalid";

  function handleChange(value: string) {
    const digits = value.replace(/\D/g, "").slice(0, 6);
    setCode(digits);
    if (digits.length === 6 && !verify.isPending) verify.mutate(digits);
  }

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (code.length === 6) verify.mutate(code);
      }}
    >
      <button
        type="button"
        onClick={onBack}
        className="-ml-1 mb-4 inline-flex items-center gap-1.5 rounded text-sm text-fg-muted hover:text-fg"
      >
        <ArrowLeft className="size-4" /> Use a different email
      </button>
      <h1 className="text-xl font-semibold">Check your email</h1>
      <p className="mt-1.5 text-sm text-fg-muted">
        We sent a sign-in link and a 6-digit code to <span className="font-medium text-fg">{email}</span>. Open
        the link, or enter the code here.
      </p>

      <label htmlFor="code" className="sr-only">
        6-digit code
      </label>
      <input
        id="code"
        autoFocus
        inputMode="numeric"
        autoComplete="one-time-code"
        pattern="[0-9]{6}"
        maxLength={6}
        value={code}
        onChange={(e) => handleChange(e.target.value)}
        placeholder="••••••"
        disabled={verify.isPending}
        className="mt-6 h-14 w-full rounded-lg bg-surface-2 text-center font-mono text-2xl tracking-[0.5em] ring-1 ring-line outline-none placeholder:text-fg-subtle focus:ring-2 focus:ring-accent disabled:opacity-60"
      />

      {verify.isError && (
        <ErrorText>{expired ? "That code has expired or was used too many times. Send a new one." : errorMessage(verify.error)}</ErrorText>
      )}
      {resend.isError && <ErrorText>{errorMessage(resend.error)}</ErrorText>}
      {resentNotice && !verify.isError && !resend.isError && (
        <p className="mt-3 text-sm text-fg-muted">We sent a new code. Earlier codes still work until they expire.</p>
      )}

      <Button type="submit" className="mt-5 w-full" disabled={code.length !== 6 || verify.isPending}>
        {verify.isPending && <Spinner className="size-4 text-accent-fg" />}
        Sign in
      </Button>
      <Button
        type="button"
        variant="ghost"
        className="mt-2 w-full"
        onClick={() => resend.mutate()}
        disabled={resend.isPending}
      >
        {resend.isPending ? "Sending…" : "Send a new code"}
      </Button>
    </form>
  );
}

function ErrorText({ children }: { children: React.ReactNode }) {
  return (
    <p role="alert" className="mt-3 text-sm text-danger">
      {children}
    </p>
  );
}
