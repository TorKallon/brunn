import {
  createContext,
  type PropsWithChildren,
  useContext,
  useEffect,
  useMemo,
} from "react";
import { createApiClient, type BrunnApi } from "./api";

export const AUTH_SESSION_QUERY_KEY = ["auth", "session"] as const;
const ApiContext = createContext<BrunnApi | null>(null);

export function AuthProvider({ children }: PropsWithChildren) {
  const api = useMemo(() => createApiClient(), []);

  useEffect(() => {
    try {
      window.sessionStorage.removeItem("brunn.access_token");
    } catch {
      // Hardened browser policies may disable storage. The legacy value is
      // never read or used by the cookie-session client.
    }
  }, []);

  return (
    <ApiContext.Provider value={api}>{children}</ApiContext.Provider>
  );
}

export function useApi(): BrunnApi {
  const value = useContext(ApiContext);
  if (!value) throw new Error("useApi must be used inside AuthProvider");
  return value;
}
