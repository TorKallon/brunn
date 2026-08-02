import {
  Outlet,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
  type RouterHistory,
} from "@tanstack/react-router";
import { AuthBoundary } from "./components/AuthBoundary";
import { EmptyState } from "./components/StateViews";
import { AssetsPage } from "./pages/AssetsPage";
import { BriefingEditionPage } from "./pages/BriefingEditionPage";
import { BriefingsPage } from "./pages/BriefingsPage";
import { CapturePage } from "./pages/CapturePage";
import { ControlPage } from "./pages/ControlPage";
import { DreamsPage } from "./pages/DreamsPage";
import { ExplorePage } from "./pages/ExplorePage";
import { TopicsPage } from "./pages/TopicsPage";
import { WorkPage } from "./pages/WorkPage";
import { ForgotPasswordPage, LoginPage, ResetPasswordPage } from "./pages/AuthPages";

function RootLayout() {
  return <Outlet />;
}

function ProtectedLayout() {
  return <AuthBoundary><Outlet /></AuthBoundary>;
}

function NotFound() {
  return (
    <main className="page">
      <EmptyState title="Route not found" />
    </main>
  );
}

const rootRoute = createRootRoute({ component: RootLayout, notFoundComponent: NotFound });
export interface LoginSearch {
  redirect?: string;
}
const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: LoginPage,
  validateSearch: (search: Record<string, unknown>): LoginSearch => ({
    redirect: safeInternalRedirect(search.redirect),
  }),
});
const forgotPasswordRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/forgot-password",
  component: ForgotPasswordPage,
});
const resetPasswordRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/reset-password",
  component: ResetPasswordPage,
});
const protectedRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: "authenticated",
  component: ProtectedLayout,
});
const indexRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/",
  beforeLoad: () => {
    throw redirect({ to: "/work" });
  },
});
const workRoute = createRoute({ getParentRoute: () => protectedRoute, path: "/work", component: WorkPage });
const briefingsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/briefings",
  component: BriefingsPage,
});
export interface BriefingEditionSearch {
  edition: string;
}
const briefingEditionRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/briefings/$date",
  component: BriefingEditionPage,
  validateSearch: (search: Record<string, unknown>): BriefingEditionSearch => ({
    edition:
      typeof search.edition === "string" && search.edition.trim()
        ? search.edition
        : "morning",
  }),
});
const topicsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/topics",
  component: TopicsPage,
});
const assetsRoute = createRoute({ getParentRoute: () => protectedRoute, path: "/assets", component: AssetsPage });
const sessionRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/sessions/$sessionId",
  beforeLoad: () => {
    throw redirect({ to: "/work" });
  },
});
const sessionAssetsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/assets/$sessionId",
  beforeLoad: () => {
    throw redirect({ to: "/assets" });
  },
});
const exploreRoute = createRoute({ getParentRoute: () => protectedRoute, path: "/explore", component: ExplorePage });
const objectRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/objects/$objectId",
  beforeLoad: () => {
    throw redirect({ to: "/explore" });
  },
});
const sourceRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/sources/$sourceId",
  beforeLoad: () => {
    throw redirect({ to: "/explore" });
  },
});
const captureRoute = createRoute({ getParentRoute: () => protectedRoute, path: "/capture", component: CapturePage });
const dreamsRoute = createRoute({ getParentRoute: () => protectedRoute, path: "/dreams", component: DreamsPage });
const dreamDetailRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/dreams/$dreamId",
  beforeLoad: () => {
    throw redirect({ to: "/dreams" });
  },
});
const controlRoute = createRoute({ getParentRoute: () => protectedRoute, path: "/control", component: ControlPage });

const routeTree = rootRoute.addChildren([
  loginRoute,
  forgotPasswordRoute,
  resetPasswordRoute,
  protectedRoute.addChildren([
    indexRoute,
    briefingsRoute,
    briefingEditionRoute,
    topicsRoute,
    workRoute,
    sessionRoute,
    assetsRoute,
    sessionAssetsRoute,
    exploreRoute,
    objectRoute,
    sourceRoute,
    captureRoute,
    dreamsRoute,
    dreamDetailRoute,
    controlRoute,
  ]),
]);

function safeInternalRedirect(value: unknown): string | undefined {
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//")) {
    return undefined;
  }
  try {
    const url = new URL(value, "https://straylight.invalid");
    if (url.origin !== "https://straylight.invalid") return undefined;
    if (["/login", "/forgot-password", "/reset-password"].includes(url.pathname)) {
      return undefined;
    }
    return `${url.pathname}${url.search}`;
  } catch {
    return undefined;
  }
}

export function createAppRouter(history?: RouterHistory) {
  return createRouter({
    routeTree,
    history,
    defaultPreload: "intent",
    defaultPreloadStaleTime: 15_000,
    scrollRestoration: true,
  });
}

export function createTestRouter(path: string) {
  return createAppRouter(createMemoryHistory({ initialEntries: [path] }));
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createAppRouter>;
  }
}
