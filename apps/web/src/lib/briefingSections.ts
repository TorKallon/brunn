import type { BriefingSectionData } from "./types";

export interface BriefingDisplaySectionPart {
  section: BriefingSectionData;
  itemLabel: string;
}

export interface BriefingDisplaySection {
  id: string;
  title: string;
  parts: BriefingDisplaySectionPart[];
  itemCount: number;
}

const GROUPED_PARENT_TITLES = ["RTS LLC", "Hobby Projects"];

function displayTitle(title: string): {
  header: string;
  itemLabel: string;
  parent?: string;
} {
  for (const parent of GROUPED_PARENT_TITLES) {
    const prefix = `${parent} — `;
    if (!title.startsWith(prefix)) continue;
    const child = title.slice(prefix.length).trim();
    if (child) return { header: parent, itemLabel: child, parent };
  }
  return { header: title, itemLabel: title };
}

export function groupBriefingSections(
  sections: BriefingSectionData[],
): BriefingDisplaySection[] {
  const groups: BriefingDisplaySection[] = [];
  const groupIndexes = new Map<string, number>();

  for (const section of sections) {
    const title = displayTitle(section.title);
    const id = title.parent ? `parent:${title.parent}` : `topic:${section.topic}`;
    const part = { section, itemLabel: title.itemLabel };
    const existingIndex = groupIndexes.get(id);

    if (existingIndex === undefined) {
      groupIndexes.set(id, groups.length);
      groups.push({
        id,
        title: title.header,
        parts: [part],
        itemCount: section.items.length,
      });
    } else {
      const group = groups[existingIndex];
      group.parts.push(part);
      group.itemCount += section.items.length;
    }
  }

  return groups;
}
