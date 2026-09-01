import type { BrunnApi } from "./api";
import type { ServiceStatus } from "./types";

export const SERVICE_STATUS_QUERY_KEY = ["service-status"] as const;

export function serviceStatusQuery(api: Pick<BrunnApi, "status">) {
  return {
    queryKey: SERVICE_STATUS_QUERY_KEY,
    queryFn: () => api.status(),
    refetchInterval: 30_000,
    retry: 1,
  } as const;
}

export function isMessagingEnabled(status: ServiceStatus | undefined): boolean {
  return status?.feature_flags?.messaging_enabled === true;
}
