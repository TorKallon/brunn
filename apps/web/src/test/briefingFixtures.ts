export const briefingListFixture = {
  status: "complete",
  data: {
    editions: [
      {
        date: "2026-08-01",
        edition: "morning",
        path: "Briefings/2026/Morning briefing - 2026-08-01.md",
        entry_ref: "entry:aug1",
        version: 2,
        generated_at: "2026-08-01T06:30:00Z",
        summary_md: [
          "**OpenAI ships o5** with a new eval harness.",
          "Datadog stayed quiet overnight.",
        ],
        section_titles: ["Frontier labs", "Portfolio"],
        item_count: 6,
      },
      {
        date: "2026-07-31",
        edition: "morning",
        path: "Briefings/2026/Morning briefing - 2026-07-31.md",
        entry_ref: "entry:jul31",
        version: 1,
        generated_at: "2026-07-31T06:30:00Z",
        summary_md: ["Quiet day across the board."],
        section_titles: ["Frontier labs"],
        item_count: 2,
      },
    ],
    limit: 14,
    truncated: false,
    next: null,
    workspace_generation: 40,
  },
};

export const briefingEditionFixture = {
  status: "complete",
  data: {
    path: "Briefings/2026/Morning briefing - 2026-08-01.md",
    entry_ref: "entry:aug1",
    version: 2,
    current_version: 2,
    date: "2026-08-01",
    edition: "morning",
    briefing: {
      schema: "briefing.v1",
      date: "2026-08-01",
      edition: "morning",
      timezone: "America/Los_Angeles",
      generated_at: "2026-08-01T06:30:00Z",
      summary_md: [
        "**OpenAI ships o5** with a new eval harness.",
        "NVDA up 3.1% on earnings.",
        "Sleep score 82, HRV steady.",
        "Discord digest: two frontier-lab threads worth reading.",
        "Datadog stayed quiet overnight.",
      ],
      sections: [
        {
          topic: "frontier-labs",
          title: "Frontier labs",
          items: [
            {
              id: "openai-o5",
              kind: "news",
              headline_md: "**OpenAI ships o5** with a new eval harness",
              delta: "new",
              detail_md:
                "o5 posts state-of-the-art results on long-horizon agentic tasks.",
              what_changed: "First release since o4-mini.",
              story: {
                key: "openai-o5-launch",
                urls: ["https://openai.com/blog/o5"],
                title: "OpenAI o5 launch",
              },
              times: {
                published_at: "2026-08-01T04:10:00Z",
                first_seen_at: "2026-08-01T05:02:00Z",
              },
            },
            {
              id: "anthropic-eval",
              kind: "news",
              headline_md: "Anthropic publishes agent-safety evals",
              delta: "update",
              detail_md: "Expanded coverage of tool-use failure modes.",
            },
          ],
        },
        {
          topic: "portfolio",
          title: "Portfolio",
          items: [
            {
              id: "nvda-earnings",
              kind: "metric",
              headline_md: "NVDA up 3.1% after earnings",
              delta: "corroboration",
              detail_md: "Data-center revenue beat consensus.",
            },
          ],
        },
      ],
      omitted: [],
      delta: { added: [], changed: [], removed: [] },
    },
    markdown: "# Morning briefing - 2026-08-01",
    created_at: "2026-08-01T06:30:00Z",
    versions: [
      { version: 1, created_at: "2026-08-01T06:30:00Z" },
      { version: 2, created_at: "2026-08-01T10:00:00Z" },
    ],
    workspace_generation: 40,
  },
};

export const legacyEditionFixture = {
  status: "complete",
  data: {
    path: "Briefings/2026/Morning briefing - 2026-07-31.md",
    entry_ref: "entry:jul31",
    version: 1,
    current_version: 1,
    date: "2026-07-31",
    edition: "morning",
    briefing: null,
    markdown:
      "# Morning briefing - 2026-07-31\n\nLegacy body without a structured payload.",
    created_at: "2026-07-31T06:30:00Z",
    versions: [{ version: 1, created_at: "2026-07-31T06:30:00Z" }],
    workspace_generation: 39,
  },
};

export const briefingTopicsFixture = {
  status: "complete",
  data: {
    topics: [
      {
        slug: "frontier-labs",
        name: "Frontier labs",
        section_order: 10,
        mode: "every_briefing",
        editions: ["morning"],
        schedule: null,
        entities: ["OpenAI", "Anthropic"],
        symbols: [],
        suppress_unchanged: true,
        freshness_hours: 48,
        body: "Track model releases and safety papers.",
        path: "Briefings/Topics/frontier-labs.md",
        entry_ref: "entry:topic-frontier",
        version: 3,
      },
      {
        slug: "stocks",
        name: "Stock watchlist",
        section_order: 70,
        mode: "on_material_delta",
        editions: ["morning", "health-update"],
        schedule: "10:00 America/Los_Angeles",
        entities: ["Alphabet", "Microsoft"],
        symbols: ["GOOGL", "MSFT"],
        suppress_unchanged: false,
        freshness_hours: 24,
        body: "Absolute dollar changes, not percentages.",
        path: "Briefings/Topics/stocks.md",
        entry_ref: "entry:topic-stocks",
        version: 5,
      },
      {
        slug: "broken",
        name: "broken",
        section_order: 1000,
        mode: "every_briefing",
        editions: ["morning"],
        schedule: null,
        entities: [],
        symbols: [],
        suppress_unchanged: true,
        freshness_hours: 48,
        body: "kind: briefing_topic\nno closing fence\n",
        parse_error:
          "frontmatter is missing or not closed; raw content preserved as body",
        path: "Briefings/Topics/broken.md",
        entry_ref: "entry:topic-broken",
        version: 1,
      },
    ],
    pending_requests: [
      {
        path: "Briefings/Requests/2026-08-01 - openai-o5.md",
        entry_ref: "entry:request-1",
        date: "2026-08-01",
        item_id: "openai-o5",
        edition_ref: "entry:aug1",
        topic: "frontier-labs",
        note: "Wants eval-harness detail",
      },
    ],
    feedback_path: "Briefings/Feedback/2026-08.md",
    feedback_tail: [
      "2026-08-01 useful openai-o5",
      "2026-08-01 repeated nvda-earnings",
    ],
    workspace_generation: 41,
  },
};
