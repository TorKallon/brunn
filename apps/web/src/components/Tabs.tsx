import type { ReactNode } from "react";

export interface TabDefinition {
  id: string;
  label: string;
  count?: number;
}

export function Tabs({
  tabs,
  active,
  onChange,
}: {
  tabs: TabDefinition[];
  active: string;
  onChange: (id: string) => void;
}) {
  return (
    <div className="tabs" role="tablist">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          role="tab"
          aria-selected={tab.id === active}
          className={tab.id === active ? "active" : undefined}
          onClick={() => onChange(tab.id)}
        >
          {tab.label}
          {tab.count !== undefined ? <span>{tab.count}</span> : null}
        </button>
      ))}
    </div>
  );
}

export function TabPanel({ id, active, children }: { id: string; active: string; children: ReactNode }) {
  if (id !== active) return null;
  return (
    <div role="tabpanel" className="tab-panel">
      {children}
    </div>
  );
}
