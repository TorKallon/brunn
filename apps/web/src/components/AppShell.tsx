import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useRouterState } from "@tanstack/react-router";
import {
  Activity,
  Bell,
  Bot,
  ChevronDown,
  CircleUserRound,
  LogOut,
  LayoutDashboard,
  Menu,
  Search,
  Settings,
  Sunrise,
  X,
} from "lucide-react";
import { type PropsWithChildren, useState } from "react";
import { ApiError } from "../lib/api";
import { useApi } from "../lib/auth";
import { useCurrent, useReadOnly } from "../lib/current";
import { formatRelative } from "../lib/format";
import { isMessagingEnabled, serviceStatusQuery } from "../lib/serviceStatus";
import { ReadOnlyNotice, StatusBadge } from "./StateViews";

const navItems = [
  { to: "/dashboard", label: "Overview", icon: LayoutDashboard },
  { to: "/alerts", label: "Alerts", icon: Bell },
  { to: "/briefings", label: "Briefings", icon: Sunrise },
  { to: "/explore", label: "Search", icon: Search },
  { to: "/control", label: "Detailed Activity", icon: Activity },
] as const;

const messagingNavItems = [
  navItems[0],
  navItems[1],
  { to: "/agents", label: "Agents", icon: Bot },
  ...navItems.slice(2),
] as const;

export function AppShell({ children }: PropsWithChildren) {
  const [navOpen, setNavOpen] = useState(false);
  const [userOpen, setUserOpen] = useState(false);
  const api = useApi();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const current = useCurrent();
  const readOnly = useReadOnly();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const statusQuery = useQuery(serviceStatusQuery(api));
  const me = current.data;
  const freshness = me.freshness ?? current.freshness;
  const serviceStatus = statusQuery.data?.data.status ?? (statusQuery.isError ? "unavailable" : "checking");
  const visibleNavItems = isMessagingEnabled(statusQuery.data?.data)
    ? messagingNavItems
    : navItems;
  const logoutMutation = useMutation({
    mutationFn: () => api.logout(),
    onSuccess: async () => {
      queryClient.clear();
      await navigate({ to: "/login", replace: true });
    },
    onError: async (error) => {
      if (error instanceof ApiError && error.status === 401) {
        queryClient.clear();
        await navigate({ to: "/login", replace: true });
      }
    },
  });

  return (
    <div className="app-layout">
      <button
        className="mobile-nav-toggle icon-button"
        type="button"
        onClick={() => setNavOpen((value) => !value)}
        aria-label={navOpen ? "Close navigation" : "Open navigation"}
        aria-expanded={navOpen}
      >
        {navOpen ? <X size={20} /> : <Menu size={20} />}
      </button>
      {navOpen ? <button className="nav-backdrop" onClick={() => setNavOpen(false)} aria-label="Close navigation" /> : null}
      <aside className={`sidebar ${navOpen ? "open" : ""}`}>
        <Link to="/dashboard" className="brand" onClick={() => setNavOpen(false)}>
          <img className="brand-mark" src="/brunn-well-128.webp" alt="" aria-hidden="true" />
          <div>
            <strong>brunn</strong>
            <span>Memory &amp; briefings</span>
          </div>
        </Link>
        <nav className="primary-nav" aria-label="Primary navigation">
          {visibleNavItems.map((item) => {
            const Icon = item.icon;
            const active = pathname === item.to || pathname.startsWith(`${item.to}/`);
            return (
              <Link
                key={item.to}
                to={item.to}
                className={active ? "active" : undefined}
                onClick={() => setNavOpen(false)}
              >
                <Icon size={18} aria-hidden="true" />
                <span>{item.label}</span>
              </Link>
            );
          })}
        </nav>
        <div className="sidebar-status">
          <div>
            <Activity size={15} aria-hidden="true" />
            <span>Service</span>
            <StatusBadge status={serviceStatus} />
          </div>
          <div>
            <CircleUserRound size={15} aria-hidden="true" />
            <span>Access</span>
            <strong>{readOnly ? "Read only" : "Read/write"}</strong>
          </div>
        </div>
      </aside>

      <div className="app-column">
        <header className="topbar">
          <div className="context-strip">
            <div>
              <span>Scope</span>
              <strong>{me.active_scope?.name ?? "All authorized"}</strong>
            </div>
            <div>
              <span>Source sync</span>
              <strong>{formatRelative(freshness?.source_updated_at)}</strong>
            </div>
            <div>
              <span>Search index</span>
              <strong>{formatRelative(freshness?.semantic_index_updated_at)}</strong>
            </div>
            {current.status !== "complete" ? <StatusBadge status={current.status} /> : null}
            {readOnly ? <ReadOnlyNotice /> : null}
          </div>
          <div className="user-menu-wrap">
            <button
              className="user-menu-trigger"
              type="button"
              onClick={() => setUserOpen((value) => !value)}
              aria-label={`User menu for ${me.user.display_name}`}
              aria-haspopup="menu"
              aria-expanded={userOpen}
            >
              <CircleUserRound size={19} aria-hidden="true" />
              <span>{me.user.display_name}</span>
              <ChevronDown size={15} aria-hidden="true" />
            </button>
            {userOpen ? (
              <div className="user-menu" role="menu">
                <div>
                  <strong>{me.user.display_name}</strong>
                  {me.user.email ? <span>{me.user.email}</span> : null}
                </div>
                <Link
                  className="user-menu-link"
                  to="/settings"
                  role="menuitem"
                  onClick={() => setUserOpen(false)}
                >
                  <Settings size={16} aria-hidden="true" />
                  Settings
                </Link>
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => logoutMutation.mutate()}
                  disabled={logoutMutation.isPending}
                >
                  <LogOut size={16} aria-hidden="true" />
                  {logoutMutation.isPending ? "Signing out…" : "Sign out"}
                </button>
                {logoutMutation.isError && !(logoutMutation.error instanceof ApiError && logoutMutation.error.status === 401) ? (
                  <p className="field-error user-menu-error" role="alert">
                    Sign-out failed. Check your connection and try again.
                  </p>
                ) : null}
              </div>
            ) : null}
          </div>
        </header>
        <div className="app-content">{children}</div>
      </div>
    </div>
  );
}
