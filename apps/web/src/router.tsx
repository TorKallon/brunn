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
import { AlertDetailPage, AlertsPage } from "./pages/AlertsPage";
import { BriefingEditionPage } from "./pages/BriefingEditionPage";
import { BriefingsPage } from "./pages/BriefingsPage";
import { CapturePage } from "./pages/CapturePage";
import { ControlPage } from "./pages/ControlPage";
import { DreamsPage } from "./pages/DreamsPage";
import { DashboardPage } from "./pages/DashboardPage";
import { DocumentPage } from "./pages/DocumentPage";
import { ExplorePage } from "./pages/ExplorePage";
import { SettingsPage } from "./pages/SettingsPage";
import { ProjectDetailPage } from "./pages/ProjectDetailPage";
import { TaskDetailPage } from "./pages/TaskDetailPage";
import { TasksPage } from "./pages/TasksPage";
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
export interface ExploreSearch {
  entryRef?: string;
  entryPath?: string;
  alternatePaths?: string;
  linkTarget?: string;
  fallbackQuery?: string;
}
export interface DocumentSearch {
  version?: number;
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
    throw redirect({ to: "/dashboard" });
  },
});
const dashboardRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/dashboard",
  component: DashboardPage,
});
const tasksRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/tasks",
  component: TasksPage,
});
const taskDetailRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/tasks/$taskRef",
  component: TaskDetailPage,
});
const projectDetailRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$slug",
  component: ProjectDetailPage,
});
const workRoute = createRoute({ getParentRoute: () => protectedRoute, path: "/work", component: WorkPage });
const briefingsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/briefings",
  component: BriefingsPage,
});
const alertsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/alerts",
  component: AlertsPage,
});
const alertDetailRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/alerts/$notificationRef",
  component: AlertDetailPage,
});
export interface BriefingEditionSearch {
  edition: string;
  item?: string;
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
    item:
      typeof search.item === "string" && search.item.trim()
        ? search.item
        : undefined,
  }),
});
const documentRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/documents/$slug",
  component: DocumentPage,
  validateSearch: (search: Record<string, unknown>): DocumentSearch => ({
    version: positiveSearchInteger(search.version),
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
const exploreRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/explore",
  component: ExplorePage,
  validateSearch: (search: Record<string, unknown>): ExploreSearch => ({
    entryRef: boundedSearchString(search.entryRef),
    entryPath: boundedSearchString(search.entryPath),
    alternatePaths: boundedSearchString(search.alternatePaths, 16_000),
    linkTarget: boundedSearchString(search.linkTarget),
    fallbackQuery: boundedSearchString(search.fallbackQuery),
  }),
});
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
const settingsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/settings",
  component: SettingsPage,
});

const routeTree = rootRoute.addChildren([
  loginRoute,
  forgotPasswordRoute,
  resetPasswordRoute,
  protectedRoute.addChildren([
    indexRoute,
    dashboardRoute,
    tasksRoute,
    taskDetailRoute,
    projectDetailRoute,
    alertsRoute,
    alertDetailRoute,
    briefingsRoute,
    briefingEditionRoute,
    documentRoute,
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
    settingsRoute,
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

function boundedSearchString(value: unknown, maxLength = 4_096): string | undefined {
  return typeof value === "string" && value.length > 0 && value.length <= maxLength
    ? value
    : undefined;
}

function positiveSearchInteger(value: unknown): number | undefined {
  if (typeof value !== "number" && typeof value !== "string") return undefined;
  if (typeof value === "string" && !/^\d+$/.test(value)) return undefined;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : undefined;
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
