import { describe, expect, it } from "vitest";
import { groupBriefingSections } from "../lib/briefingSections";
import type { BriefingSectionData } from "../lib/types";

function section(topic: string, title: string): BriefingSectionData {
  return {
    topic,
    title,
    items: [{ id: topic, kind: "metric", headline_md: topic }],
  };
}

describe("briefing section display grouping", () => {
  it("groups project parents while preserving child topics and labels", () => {
    const groups = groupBriefingSections([
      section("calendar", "Today's calendar"),
      section("charlemagne", "RTS LLC — Charlemagne"),
      section("joyeuse", "RTS LLC — Joyeuse"),
      section("railway", "Hobby Projects — Railway"),
      section("ai", "AI — material updates"),
    ]);

    expect(groups.map((group) => group.title)).toEqual([
      "Today's calendar",
      "RTS LLC",
      "Hobby Projects",
      "AI — material updates",
    ]);
    expect(groups.map((group) => group.itemCount)).toEqual([1, 2, 1, 1]);
    expect(groups[1].parts.map((part) => part.itemLabel)).toEqual([
      "Charlemagne",
      "Joyeuse",
    ]);
    expect(groups[1].parts.map((part) => part.section.topic)).toEqual([
      "charlemagne",
      "joyeuse",
    ]);
    expect(groups[2].parts[0].itemLabel).toBe("Railway");
    expect(groups[3].parts[0].itemLabel).toBe("AI — material updates");
  });
});
