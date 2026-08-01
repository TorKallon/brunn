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

function RootLayout() {
  return (
    <AuthBoundary>
      <Outlet />
    </AuthBoundary>
  );
}

function NotFound() {
  return (
    <main className="page">
      <EmptyState title="Route not found" />
    </main>
  );
}

const rootRoute = createRootRoute({ component: RootLayout, notFoundComponent: NotFound });
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  beforeLoad: () => {
    throw redirect({ to: "/work" });
  },
});
const workRoute = createRoute({ getParentRoute: () => rootRoute, path: "/work", component: WorkPage });
const briefingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/briefings",
  component: BriefingsPage,
});
export interface BriefingEditionSearch {
  edition: string;
}
const briefingEditionRoute = createRoute({
  getParentRoute: () => rootRoute,
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
  getParentRoute: () => rootRoute,
  path: "/topics",
  component: TopicsPage,
});
const assetsRoute = createRoute({ getParentRoute: () => rootRoute, path: "/assets", component: AssetsPage });
const sessionRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sessions/$sessionId",
  beforeLoad: () => {
    throw redirect({ to: "/work" });
  },
});
const sessionAssetsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/assets/$sessionId",
  beforeLoad: () => {
    throw redirect({ to: "/assets" });
  },
});
const exploreRoute = createRoute({ getParentRoute: () => rootRoute, path: "/explore", component: ExplorePage });
const objectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/objects/$objectId",
  beforeLoad: () => {
    throw redirect({ to: "/explore" });
  },
});
const sourceRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sources/$sourceId",
  beforeLoad: () => {
    throw redirect({ to: "/explore" });
  },
});
const captureRoute = createRoute({ getParentRoute: () => rootRoute, path: "/capture", component: CapturePage });
const dreamsRoute = createRoute({ getParentRoute: () => rootRoute, path: "/dreams", component: DreamsPage });
const dreamDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/dreams/$dreamId",
  beforeLoad: () => {
    throw redirect({ to: "/dreams" });
  },
});
const controlRoute = createRoute({ getParentRoute: () => rootRoute, path: "/control", component: ControlPage });

const routeTree = rootRoute.addChildren([
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
]);

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
