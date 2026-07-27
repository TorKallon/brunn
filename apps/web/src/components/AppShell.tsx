import { useQuery } from "@tanstack/react-query";
import { Link, useRouterState } from "@tanstack/react-router";
import {
  Activity,
  Binary,
  ChevronDown,
  CircleUserRound,
  FilePenLine,
  FolderOpen,
  LogOut,
  Menu,
  Search,
  Sparkles,
  X,
} from "lucide-react";
import { type PropsWithChildren, useState } from "react";
import { useApi, useAuth } from "../lib/auth";
import { useCurrent, useReadOnly } from "../lib/current";
import { formatRelative } from "../lib/format";
import { ReadOnlyNotice, StatusBadge } from "./StateViews";

const navItems = [
  { to: "/work", label: "Workspace", icon: FolderOpen },
  { to: "/explore", label: "Search", icon: Search },
  { to: "/capture", label: "Write", icon: FilePenLine },
  { to: "/assets", label: "Binaries", icon: Binary },
  { to: "/dreams", label: "Background", icon: Sparkles },
  { to: "/control", label: "Activity", icon: Activity },
] as const;

export function AppShell({ children }: PropsWithChildren) {
  const [navOpen, setNavOpen] = useState(false);
  const [userOpen, setUserOpen] = useState(false);
  const { signOut } = useAuth();
  const api = useApi();
  const current = useCurrent();
  const readOnly = useReadOnly();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const statusQuery = useQuery({
    queryKey: ["service-status"],
    queryFn: () => api.status(),
    refetchInterval: 30_000,
    retry: 1,
  });
  const me = current.data;
  const freshness = me.freshness ?? current.freshness;
  const serviceStatus = statusQuery.data?.data.status ?? (statusQuery.isError ? "unavailable" : "checking");

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
        <Link to="/work" className="brand" onClick={() => setNavOpen(false)}>
          <span className="brand-mark">S</span>
          <div>
            <strong>Straylight</strong>
            <span>Workspace &amp; memory</span>
          </div>
        </Link>
        <nav className="primary-nav" aria-label="Primary navigation">
          {navItems.map((item) => {
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
              <span>Workspace</span>
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
              <div className="user-menu">
                <div>
                  <strong>{me.user.display_name}</strong>
                  {me.user.email ? <span>{me.user.email}</span> : null}
                </div>
                <button type="button" onClick={signOut}>
                  <LogOut size={16} aria-hidden="true" />
                  Sign out
                </button>
              </div>
            ) : null}
          </div>
        </header>
        <div className="app-content">{children}</div>
      </div>
    </div>
  );
}
