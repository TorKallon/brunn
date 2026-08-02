import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import {
  ArrowLeft,
  CheckCircle2,
  Eye,
  EyeOff,
  LoaderCircle,
  LockKeyhole,
  LogIn,
  Mail,
  UserRound,
} from "lucide-react";
import {
  type FormEvent,
  type ReactNode,
  useLayoutEffect,
  useState,
} from "react";
import { ApiError } from "../lib/api";
import { AUTH_SESSION_QUERY_KEY, useApi } from "../lib/auth";

const MIN_PASSWORD_LENGTH = 15;
const MAX_PASSWORD_LENGTH = 1024;

function AuthLayout({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <main className="login-layout">
      <section className="login-panel" aria-labelledby="auth-title">
        <div className="brand brand-login" aria-label="Straylight">
          <span className="brand-mark" aria-hidden="true">S</span>
          <div>
            <strong>Straylight</strong>
            <span>Workspace &amp; memory</span>
          </div>
        </div>
        <header className="auth-heading">
          <h1 id="auth-title">{title}</h1>
          <p>{description}</p>
        </header>
        {children}
      </section>
    </main>
  );
}

function PasswordField({
  id,
  label,
  value,
  onChange,
  autoComplete,
  describedBy,
  invalid = false,
  autoFocus = false,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  autoComplete: "current-password" | "new-password";
  describedBy?: string;
  invalid?: boolean;
  autoFocus?: boolean;
}) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <div className={`input-with-action ${invalid ? "input-invalid" : ""}`}>
        <LockKeyhole size={17} aria-hidden="true" />
        <input
          id={id}
          name={id}
          type={visible ? "text" : "password"}
          autoComplete={autoComplete}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          aria-invalid={invalid || undefined}
          aria-describedby={describedBy}
          autoFocus={autoFocus}
          maxLength={MAX_PASSWORD_LENGTH * 2}
          required
        />
        <button
          className="icon-button"
          type="button"
          onClick={() => setVisible((current) => !current)}
          aria-label={visible ? `Hide ${label.toLowerCase()}` : `Show ${label.toLowerCase()}`}
          title={visible ? `Hide ${label.toLowerCase()}` : `Show ${label.toLowerCase()}`}
        >
          {visible ? <EyeOff size={17} aria-hidden="true" /> : <Eye size={17} aria-hidden="true" />}
        </button>
      </div>
    </div>
  );
}

export function LoginPage() {
  const api = useApi();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const search = useSearch({ from: "/login" });
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const loginMutation = useMutation({
    mutationFn: () => api.login(username.trim(), password),
    onSuccess: async (session) => {
      queryClient.clear();
      queryClient.setQueryData(AUTH_SESSION_QUERY_KEY, session);
      const destination = search.redirect ?? "/work";
      await navigate({ to: destination as "/work", replace: true });
    },
  });
  const error = loginMutation.isError ? loginErrorMessage(loginMutation.error) : null;

  function resetError() {
    if (loginMutation.isError) loginMutation.reset();
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!username.trim() || !password || loginMutation.isPending) return;
    loginMutation.mutate();
  }

  return (
    <AuthLayout
      title="Sign in"
      description="Use your Straylight username and password."
    >
      <form className="auth-form" onSubmit={submit} aria-busy={loginMutation.isPending} noValidate>
        <div className="field">
          <label htmlFor="username">Username</label>
          <div className={`input-with-action ${error ? "input-invalid" : ""}`}>
            <UserRound size={17} aria-hidden="true" />
            <input
              id="username"
              name="username"
              type="text"
              autoComplete="username"
              autoCapitalize="none"
              spellCheck={false}
              value={username}
              onChange={(event) => {
                setUsername(event.target.value);
                resetError();
              }}
              aria-invalid={Boolean(error) || undefined}
              aria-describedby={error ? "login-error" : undefined}
              autoFocus
              maxLength={254}
              required
            />
          </div>
        </div>
        <PasswordField
          id="password"
          label="Password"
          value={password}
          onChange={(value) => {
            setPassword(value);
            resetError();
          }}
          autoComplete="current-password"
          invalid={Boolean(error)}
          describedBy={error ? "login-error" : undefined}
        />
        <div className="auth-link-row">
          <Link to="/forgot-password">Forgot password?</Link>
        </div>
        {error ? <p id="login-error" className="field-error" role="alert">{error}</p> : null}
        <button
          className="button primary login-button"
          type="submit"
          disabled={!username.trim() || !password || loginMutation.isPending}
        >
          {loginMutation.isPending ? (
            <LoaderCircle className="spin" size={17} aria-hidden="true" />
          ) : (
            <LogIn size={17} aria-hidden="true" />
          )}
          {loginMutation.isPending ? "Signing in…" : "Sign in"}
        </button>
      </form>
    </AuthLayout>
  );
}

export function ForgotPasswordPage() {
  const api = useApi();
  const [identifier, setIdentifier] = useState("");
  const requestMutation = useMutation({
    mutationFn: () => api.forgotPassword(identifier.trim()),
  });
  const error = requestMutation.isError
    ? recoveryErrorMessage(requestMutation.error, "request")
    : null;

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!identifier.trim() || requestMutation.isPending) return;
    requestMutation.mutate();
  }

  return (
    <AuthLayout
      title="Reset your password"
      description="Enter your username or account email."
    >
      {requestMutation.isSuccess ? (
        <div className="auth-result" role="status">
          <CheckCircle2 className="success-icon" size={24} aria-hidden="true" />
          <strong>Check your email</strong>
          <p>
            If an account matches that information, we sent a password reset link.
          </p>
          <button className="button secondary" type="button" onClick={() => requestMutation.reset()}>
            Try another username or email
          </button>
        </div>
      ) : (
        <form className="auth-form" onSubmit={submit} aria-busy={requestMutation.isPending} noValidate>
          <div className="field">
            <label htmlFor="recovery-identifier">Username or email</label>
            <div className={`input-with-action ${error ? "input-invalid" : ""}`}>
              <Mail size={17} aria-hidden="true" />
              <input
                id="recovery-identifier"
                name="identifier"
                type="text"
                autoComplete="username"
                autoCapitalize="none"
                spellCheck={false}
                value={identifier}
                onChange={(event) => {
                  setIdentifier(event.target.value);
                  if (requestMutation.isError) requestMutation.reset();
                }}
                aria-invalid={Boolean(error) || undefined}
                aria-describedby={error ? "recovery-error" : undefined}
                autoFocus
                maxLength={254}
                required
              />
            </div>
          </div>
          {error ? <p id="recovery-error" className="field-error" role="alert">{error}</p> : null}
          <button
            className="button primary login-button"
            type="submit"
            disabled={!identifier.trim() || requestMutation.isPending}
          >
            {requestMutation.isPending ? <LoaderCircle className="spin" size={17} aria-hidden="true" /> : <Mail size={17} aria-hidden="true" />}
            {requestMutation.isPending ? "Sending…" : "Send reset link"}
          </button>
        </form>
      )}
      <div className="auth-footer">
        <Link to="/login"><ArrowLeft size={15} aria-hidden="true" /> Back to sign in</Link>
      </div>
    </AuthLayout>
  );
}

export function ResetPasswordPage() {
  const api = useApi();
  const queryClient = useQueryClient();
  const [token, setToken] = useState(readResetToken);
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [validationError, setValidationError] = useState<string | null>(null);
  const [completed, setCompleted] = useState(false);
  const resetMutation = useMutation({
    mutationFn: () => api.resetPassword(token, password.normalize("NFC")),
    onSuccess: () => {
      queryClient.clear();
      setToken("");
      setPassword("");
      setConfirmation("");
      setCompleted(true);
      resetMutation.reset();
    },
  });
  const requestError = resetMutation.isError
    ? recoveryErrorMessage(resetMutation.error, "reset")
    : null;
  const error = validationError ?? requestError;

  useLayoutEffect(() => {
    scrubResetTokenFragment();
  }, []);

  function clearError() {
    setValidationError(null);
    if (resetMutation.isError) resetMutation.reset();
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (resetMutation.isPending) return;
    const normalizedPassword = password.normalize("NFC");
    if (Array.from(normalizedPassword).length < MIN_PASSWORD_LENGTH) {
      setValidationError(`Use at least ${MIN_PASSWORD_LENGTH} characters.`);
      return;
    }
    if (Array.from(normalizedPassword).length > MAX_PASSWORD_LENGTH) {
      setValidationError(`Use no more than ${MAX_PASSWORD_LENGTH} characters.`);
      return;
    }
    if (normalizedPassword !== confirmation.normalize("NFC")) {
      setValidationError("The passwords do not match.");
      return;
    }
    resetMutation.mutate();
  }

  if (completed) {
    return (
      <AuthLayout title="Password updated" description="Your old web sessions have been signed out.">
        <div className="auth-result" role="status">
          <CheckCircle2 className="success-icon" size={24} aria-hidden="true" />
          <p>You can now sign in with your new password.</p>
          <Link className="button primary" to="/login">Sign in</Link>
        </div>
      </AuthLayout>
    );
  }

  if (!token) {
    return (
      <AuthLayout title="Reset link unavailable" description="This page needs a valid password reset link.">
        <div className="auth-result">
          <p className="field-error" role="alert">
            The reset link is missing, invalid, or has already been removed from this browser.
          </p>
          <Link className="button primary" to="/forgot-password">Request a new link</Link>
        </div>
        <div className="auth-footer">
          <Link to="/login"><ArrowLeft size={15} aria-hidden="true" /> Back to sign in</Link>
        </div>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout title="Choose a new password" description="Set a new password for your Straylight account.">
      <form className="auth-form" onSubmit={submit} aria-busy={resetMutation.isPending} noValidate>
        <PasswordField
          id="new-password"
          label="New password"
          value={password}
          onChange={(value) => {
            setPassword(value);
            clearError();
          }}
          autoComplete="new-password"
          describedBy={`new-password-help${error ? " reset-error" : ""}`}
          invalid={Boolean(error)}
          autoFocus
        />
        <p id="new-password-help" className="auth-help">
          Use at least {MIN_PASSWORD_LENGTH} characters and avoid common passwords.
        </p>
        <PasswordField
          id="confirm-password"
          label="Confirm new password"
          value={confirmation}
          onChange={(value) => {
            setConfirmation(value);
            clearError();
          }}
          autoComplete="new-password"
          describedBy={error ? "reset-error" : undefined}
          invalid={Boolean(error)}
        />
        {error ? <p id="reset-error" className="field-error" role="alert">{error}</p> : null}
        <button
          className="button primary login-button"
          type="submit"
          disabled={!password || !confirmation || resetMutation.isPending}
        >
          {resetMutation.isPending ? <LoaderCircle className="spin" size={17} aria-hidden="true" /> : <LockKeyhole size={17} aria-hidden="true" />}
          {resetMutation.isPending ? "Updating…" : "Update password"}
        </button>
      </form>
      <div className="auth-footer">
        <Link to="/login"><ArrowLeft size={15} aria-hidden="true" /> Back to sign in</Link>
      </div>
    </AuthLayout>
  );
}

export function readResetToken(): string {
  const fragment = window.location.hash.startsWith("#")
    ? window.location.hash.slice(1)
    : window.location.hash;
  return new URLSearchParams(fragment).get("token")?.trim() ?? "";
}

function scrubResetTokenFragment(): void {
  if (window.location.hash) {
    window.history.replaceState(
      window.history.state,
      "",
      `${window.location.pathname}${window.location.search}`,
    );
  }
}

function loginErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.status === 401) return "The username or password is incorrect.";
    if (error.status === 429) return "Too many sign-in attempts. Wait a moment and try again.";
    if (error.status === 0) return "Straylight could not be reached. Check your connection and try again.";
  }
  return "Sign-in is temporarily unavailable. Try again shortly.";
}

function recoveryErrorMessage(error: unknown, action: "request" | "reset"): string {
  if (error instanceof ApiError) {
    if (error.status === 429) return "Too many requests. Wait a moment and try again.";
    if (error.status === 0) return "Straylight could not be reached. Check your connection and try again.";
    if (
      action === "reset"
      && (error.code === "invalid_password_reset" || [401, 404, 410].includes(error.status))
    ) {
      return "This reset link is invalid or has expired. Request a new link.";
    }
    if (
      action === "reset"
      && (error.code === "password_policy_violation" || error.status === 422)
    ) {
      return `Use ${MIN_PASSWORD_LENGTH} to ${MAX_PASSWORD_LENGTH} characters and choose a password that is not commonly used.`;
    }
  }
  return action === "request"
    ? "We could not request a reset link. Try again shortly."
    : "We could not update the password. Try again shortly.";
}
