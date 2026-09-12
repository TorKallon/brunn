export const publishedDocumentFixture = {
  status: "complete",
  data: {
    slug: "switzerland-itinerary",
    title: "Switzerland summer itinerary",
    summary: "A polished two-week plan with a quiet first day and flexible mountain weather options.",
    sources: [
      {
        label: "Swiss Federal Railways",
        url: "https://www.sbb.ch/en",
      },
      {
        label: "Trip planning notes",
        entry_ref: "entry:11111111-1111-4111-8111-111111111111",
      },
      {
        label: "Unsafe source",
        url: "javascript:alert(1)",
      },
      {
        label: "Credential-bearing source",
        url: "https://username:password@example.com/private",
      },
    ],
    body_md:
      "## First week\n\nStart in **Zürich**, then take the train to Lucerne. " +
      "[Check service](https://example.com/trains).\n\n" +
      "| Day | Base |\n| --- | --- |\n| 1 | Zürich |\n\n" +
      "<script>window.pwned = true;</script>",
    markdown:
      "# Switzerland summer itinerary\n\n## First week\n\nStart in **Zürich**.",
    version: 3,
    current_version: 3,
    published_at: "2026-08-06T17:00:00Z",
    updated_at: "2026-08-08T19:30:00Z",
    versions: [
      {
        version: 1,
        created_at: "2026-08-06T17:00:00Z",
        version_url: "https://brunn.test/documents/switzerland-itinerary?version=1",
      },
      {
        version: 2,
        created_at: "2026-08-07T18:00:00Z",
        version_url: "https://brunn.test/documents/switzerland-itinerary?version=2",
      },
      {
        version: 3,
        created_at: "2026-08-08T19:30:00Z",
        version_url: "https://brunn.test/documents/switzerland-itinerary?version=3",
      },
    ],
    url: "https://brunn.test/documents/switzerland-itinerary",
    version_url: "https://brunn.test/documents/switzerland-itinerary?version=3",
    workspace_generation: 42,
  },
};

/** The static build-served document: no entry, no versions, no sources. */
export const agentOrientationDocumentFixture = {
  status: "complete",
  data: {
    slug: "agent-orientation",
    title: "Agent orientation",
    summary: "Background rules that tool schemas cannot state.",
    sources: [],
    body_md: "## Before writing\n\nRead the project hub before adding tasks.",
    markdown: "# Agent orientation\n\n## Before writing\n\nRead the project hub before adding tasks.",
    path: null,
    entry_ref: null,
    version_ref: null,
    version: 1,
    current_version: 1,
    published_at: null,
    updated_at: null,
    versions: [],
    url: "https://brunn.test/documents/agent-orientation",
    version_url: "https://brunn.test/documents/agent-orientation?version=1",
    app_url: "https://brunn.test/app/documents/agent-orientation",
    app_version_url: "https://brunn.test/app/documents/agent-orientation?version=1",
  },
};

export function historicalPublishedDocumentFixture(version = 2) {
  return {
    ...publishedDocumentFixture,
    data: {
      ...publishedDocumentFixture.data,
      version,
      updated_at: "2026-08-07T18:00:00Z",
      version_url: `https://brunn.test/documents/switzerland-itinerary?version=${version}`,
    },
  };
}
