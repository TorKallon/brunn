import { createContext, type PropsWithChildren, useContext } from "react";
import type { ApiEnvelope, MeData } from "./types";

const CurrentContext = createContext<ApiEnvelope<MeData> | null>(null);

export function CurrentProvider({
  value,
  children,
}: PropsWithChildren<{ value: ApiEnvelope<MeData> }>) {
  return <CurrentContext.Provider value={value}>{children}</CurrentContext.Provider>;
}

export function useCurrent(): ApiEnvelope<MeData> {
  const value = useContext(CurrentContext);
  if (!value) throw new Error("useCurrent must be used inside CurrentProvider");
  return value;
}

export function useReadOnly(): boolean {
  const { data } = useCurrent();
  return Boolean(
    data.read_only ||
      data.active_scope?.access === "read_only" ||
      !data.capabilities?.some((capability) =>
        [
          "save",
          "checkpoint",
          "stage",
          "correct",
          "delete",
          "dream",
          "write",
          "read_write",
          "memory.save",
        ].includes(capability),
      ),
  );
}

export function useCapability(capability: string): boolean {
  return Boolean(useCurrent().data.capabilities?.includes(capability));
}
