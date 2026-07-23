import {
  Outlet,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
  useParams,
  type RouterHistory,
} from "@tanstack/react-router";
import { AuthBoundary } from "./components/AuthBoundary";
import { EmptyState } from "./components/StateViews";
import { CapturePage } from "./pages/CapturePage";
import { ControlPage } from "./pages/ControlPage";
import { DreamsPage } from "./pages/DreamsPage";
import { ExplorePage } from "./pages/ExplorePage";
import { ObjectPage } from "./pages/ObjectPage";
import { SourcePage } from "./pages/SourcePage";
import { WorkPage } from "./pages/WorkPage";
import { WorkspacePage } from "./pages/WorkspacePage";

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
const sessionRoute = createRoute({ getParentRoute: () => rootRoute, path: "/sessions/$sessionId", component: WorkspacePage });
const exploreRoute = createRoute({ getParentRoute: () => rootRoute, path: "/explore", component: ExplorePage });
const objectRoute = createRoute({ getParentRoute: () => rootRoute, path: "/objects/$objectId", component: ObjectPage });
const sourceRoute = createRoute({ getParentRoute: () => rootRoute, path: "/sources/$sourceId", component: SourcePage });
const captureRoute = createRoute({ getParentRoute: () => rootRoute, path: "/capture", component: CapturePage });
const dreamsRoute = createRoute({ getParentRoute: () => rootRoute, path: "/dreams", component: DreamsPage });

function DreamDetailRoute() {
  const { dreamId } = useParams({ from: "/dreams/$dreamId" });
  return <DreamsPage dreamId={dreamId} />;
}

const dreamDetailRoute = createRoute({ getParentRoute: () => rootRoute, path: "/dreams/$dreamId", component: DreamDetailRoute });
const controlRoute = createRoute({ getParentRoute: () => rootRoute, path: "/control", component: ControlPage });

const routeTree = rootRoute.addChildren([
  indexRoute,
  workRoute,
  sessionRoute,
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
