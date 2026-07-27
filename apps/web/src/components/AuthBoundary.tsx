import { useQuery } from "@tanstack/react-query";
import { Eye, EyeOff, KeyRound, LogIn, RefreshCw } from "lucide-react";
import { type FormEvent, type PropsWithChildren, useState } from "react";
import { ApiError } from "../lib/api";
import { useApi, useAuth } from "../lib/auth";
import { CurrentProvider } from "../lib/current";
import { AppShell } from "./AppShell";
import { ErrorState, LoadingState } from "./StateViews";

export function LoginScreen({ error }: { error?: string }) {
  const { authenticate } = useAuth();
  const [token, setToken] = useState("");
  const [visible, setVisible] = useState(false);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (token.trim()) authenticate(token);
  }

  return (
    <main className="login-layout">
      <form className="login-panel" onSubmit={submit}>
        <div className="brand brand-login">
          <span className="brand-mark">S</span>
          <div>
            <h1>Straylight</h1>
            <span>Workspace &amp; memory</span>
          </div>
        </div>
        <label htmlFor="access-token">Access token</label>
        <div className="input-with-action">
          <KeyRound size={17} aria-hidden="true" />
          <input
            id="access-token"
            name="access-token"
            type={visible ? "text" : "password"}
            autoComplete="off"
            spellCheck={false}
            value={token}
            onChange={(event) => setToken(event.target.value)}
            autoFocus
          />
          <button
            className="icon-button"
            type="button"
            onClick={() => setVisible((value) => !value)}
            aria-label={visible ? "Hide token" : "Show token"}
            title={visible ? "Hide token" : "Show token"}
          >
            {visible ? <EyeOff size={17} /> : <Eye size={17} />}
          </button>
        </div>
        {error ? <p className="field-error" role="alert">{error}</p> : null}
        <button className="button primary login-button" type="submit" disabled={!token.trim()}>
          <LogIn size={17} aria-hidden="true" />
          Connect
        </button>
      </form>
    </main>
  );
}

export function AuthBoundary({ children }: PropsWithChildren) {
  const { token, signOut } = useAuth();
  const api = useApi();
  const meQuery = useQuery({
    queryKey: ["me", token],
    queryFn: () => api.me(),
    enabled: Boolean(token),
    retry: (count, error) => !(error instanceof ApiError && error.status === 401) && count < 2,
  });

  if (!token) return <LoginScreen />;
  if (meQuery.isPending) {
    return (
      <main className="full-state">
        <LoadingState label="Connecting to Straylight" />
      </main>
    );
  }
  if (meQuery.isError) {
    if (meQuery.error instanceof ApiError && meQuery.error.status === 401) {
      return <LoginScreen error="That token is not valid or has expired." />;
    }
    return (
      <main className="full-state">
        <ErrorState error={meQuery.error} retry={() => void meQuery.refetch()} title="Connection failed" />
        <button className="button secondary" type="button" onClick={signOut}>
          <RefreshCw size={16} aria-hidden="true" />
          Use another token
        </button>
      </main>
    );
  }

  return (
    <CurrentProvider value={meQuery.data}>
      <AppShell>{children}</AppShell>
    </CurrentProvider>
  );
}
