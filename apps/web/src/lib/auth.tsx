import {
  createContext,
  type PropsWithChildren,
  useCallback,
  useContext,
  useMemo,
  useState,
} from "react";
import { createApiClient, type StraylightApi } from "./api";

export const AUTH_SESSION_KEY = "straylight.access_token";

interface AuthContextValue {
  token: string | null;
  authenticate: (token: string) => void;
  signOut: () => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);
const ApiContext = createContext<StraylightApi | null>(null);

export function AuthProvider({ children }: PropsWithChildren) {
  const [token, setToken] = useState<string | null>(() =>
    window.sessionStorage.getItem(AUTH_SESSION_KEY),
  );

  const authenticate = useCallback((nextToken: string) => {
    const normalized = nextToken.trim();
    window.sessionStorage.setItem(AUTH_SESSION_KEY, normalized);
    setToken(normalized);
  }, []);

  const signOut = useCallback(() => {
    window.sessionStorage.removeItem(AUTH_SESSION_KEY);
    setToken(null);
  }, []);

  const authValue = useMemo(
    () => ({ token, authenticate, signOut }),
    [authenticate, signOut, token],
  );
  const api = useMemo(() => createApiClient(() => token), [token]);

  return (
    <AuthContext.Provider value={authValue}>
      <ApiContext.Provider value={api}>{children}</ApiContext.Provider>
    </AuthContext.Provider>
  );
}

export function useAuth(): AuthContextValue {
  const value = useContext(AuthContext);
  if (!value) throw new Error("useAuth must be used inside AuthProvider");
  return value;
}

export function useApi(): StraylightApi {
  const value = useContext(ApiContext);
  if (!value) throw new Error("useApi must be used inside AuthProvider");
  return value;
}
