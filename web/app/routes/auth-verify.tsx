import { CircleAlert } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Link, useLocation, useNavigate } from "react-router";

import { Logo, Spinner } from "~/components/ui";
import { api, errorMessage, unwrap } from "~/lib/api/client";
import { APP_NAME } from "~/lib/brand";
import { setSignedIn } from "~/lib/session";

export function meta() {
  return [{ title: `Signing in · ${APP_NAME}` }];
}

/**
 * Landing page for the emailed magic link: `/auth/verify#token=…`.
 * The token is in the fragment, so it never reaches a server. We POST it, because
 * link scanners in mail clients only GET, and a GET must not use up a single-use token.
 */
export default function AuthVerify() {
  const location = useLocation();
  const navigate = useNavigate();
  const [error, setError] = useState<string | null>(null);
  const started = useRef(false); // the token is single-use; guard against StrictMode double effects

  useEffect(() => {
    if (started.current) return;
    started.current = true;

    const token = new URLSearchParams(location.hash.slice(1)).get("token");
    navigate({ pathname: location.pathname, hash: "" }, { replace: true }); // drop the token from history

    if (!token) {
      setError("This sign-in link is incomplete. Try copying the whole link, or sign in with the code instead.");
      return;
    }
    api
      .POST("/api/auth/email/verify-link", { body: { token } })
      .then(unwrap)
      .then((me) => {
        setSignedIn(me);
        navigate("/", { replace: true });
      })
      .catch((err) => setError(errorMessage(err)));
  }, [location, navigate]);

  return (
    <main className="grid min-h-dvh place-items-center px-4">
      <div className="flex max-w-sm flex-col items-center text-center">
        <Logo className="mb-10 scale-125" />
        {error ? (
          <>
            <CircleAlert className="size-8 text-danger" />
            <h1 className="mt-4 text-xl font-semibold">We couldn't sign you in</h1>
            <p className="mt-2 text-sm text-fg-muted">{error}</p>
            <Link
              to="/login"
              className="mt-6 inline-flex h-10 items-center rounded-full bg-accent px-5 text-sm font-medium text-accent-fg hover:bg-accent-hover"
            >
              Back to sign in
            </Link>
          </>
        ) : (
          <>
            <Spinner />
            <p className="mt-4 text-sm text-fg-muted">Signing you in…</p>
          </>
        )}
      </div>
    </main>
  );
}
