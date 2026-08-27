import { Moon, Sun } from "lucide-react";
import { useState } from "react";
import { DefinitionList, Page, PageHeader, Section } from "../components/Page";
import { TaskSettingsPanels } from "../components/TaskSettingsPanels";
import {
  type Appearance,
  readAppearance,
  saveAppearance,
} from "../lib/appearance";
import { useCurrent } from "../lib/current";
import "../settings.css";

const appearanceOptions: Array<{
  value: Appearance;
  label: string;
  description: string;
  icon: typeof Moon;
}> = [
  {
    value: "dark",
    label: "Dark",
    description: "Low-glare Night Signal workroom",
    icon: Moon,
  },
  {
    value: "light",
    label: "Light",
    description: "Bright workroom with night navigation",
    icon: Sun,
  },
];

export function SettingsPage() {
  const current = useCurrent();
  const [appearance, setAppearance] = useState<Appearance>(readAppearance);
  const user = current.data.user;

  function chooseAppearance(next: Appearance) {
    setAppearance(next);
    saveAppearance(next);
  }

  return (
    <Page>
      <PageHeader
        title="Settings"
        description="Account details and preferences for this browser"
      />

      <Section title="Account">
        <DefinitionList
          items={[
            { label: "Name", value: user.display_name },
            { label: "Email", value: user.email ?? user.username ?? "Not available" },
          ]}
        />
      </Section>

      <Section title="Appearance">
        <fieldset className="appearance-fieldset">
          <legend>Color mode</legend>
          <div className="appearance-options">
            {appearanceOptions.map((option) => {
              const Icon = option.icon;
              return (
                <label
                  className={`appearance-option ${appearance === option.value ? "active" : ""}`.trim()}
                  key={option.value}
                >
                  <input
                    type="radio"
                    name="appearance"
                    value={option.value}
                    checked={appearance === option.value}
                    onChange={() => chooseAppearance(option.value)}
                  />
                  <Icon size={20} aria-hidden="true" />
                  <span>
                    <strong>{option.label}</strong>
                    <small>{option.description}</small>
                  </span>
                </label>
              );
            })}
          </div>
        </fieldset>
        <p className="settings-note">
          Appearance is saved in this browser and applies immediately.
        </p>
      </Section>

      <TaskSettingsPanels />
    </Page>
  );
}
