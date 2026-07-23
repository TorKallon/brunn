import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, type AnyRouter } from "@tanstack/react-router";
import { useState } from "react";
import { AuthProvider } from "./lib/auth";

export function StraylightApp({
  router,
  queryClient,
}: {
  router: AnyRouter;
  queryClient?: QueryClient;
}) {
  const [client] = useState(
    () =>
      queryClient ??
      new QueryClient({
        defaultOptions: {
          queries: {
            staleTime: 15_000,
            retry: 1,
            refetchOnWindowFocus: false,
          },
          mutations: { retry: false },
        },
      }),
  );
  return (
    <QueryClientProvider client={client}>
      <AuthProvider>
        <RouterProvider router={router} />
      </AuthProvider>
    </QueryClientProvider>
  );
}
