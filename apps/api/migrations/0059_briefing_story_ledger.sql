-- Derived, rebuildable projection of briefing.v1 edition metadata.
-- Provides dedupe uniqueness and delivery history. Canonical truth remains
-- the Briefings/ markdown entries; rebuild_briefing_ledger reconstructs
-- these tables from edition metadata at any time.

CREATE TABLE straylight.briefing_stories (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  story_key text NOT NULL CHECK (story_key ~ '^[a-z0-9][a-z0-9-]{2,79}$'),
  title text NOT NULL DEFAULT '',
  topic text NOT NULL DEFAULT '',
  entities text[] NOT NULL DEFAULT '{}',
  event_at date,
  first_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  last_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  last_delivered_date date,
  last_delivered_edition_ref text,
  last_delivered_headline text,
  delivery_count integer NOT NULL DEFAULT 0 CHECK (delivery_count >= 0),
  suppression_count integer NOT NULL DEFAULT 0 CHECK (suppression_count >= 0),
  status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'dormant')),
  PRIMARY KEY (user_id, story_key)
);

CREATE INDEX briefing_stories_user_seen_idx
  ON straylight.briefing_stories (user_id, last_seen_at DESC);

CREATE TABLE straylight.briefing_story_urls (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  url_hash straylight.sha256_hex NOT NULL,
  story_key text NOT NULL,
  url text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, url_hash),
  FOREIGN KEY (user_id, story_key)
    REFERENCES straylight.briefing_stories(user_id, story_key)
    ON DELETE CASCADE
);

CREATE INDEX briefing_story_urls_user_story_idx
  ON straylight.briefing_story_urls (user_id, story_key);

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'briefing_stories',
    'briefing_story_urls'
  ]
  LOOP
    EXECUTE format(
      'ALTER TABLE straylight.%I ENABLE ROW LEVEL SECURITY',
      table_name
    );
    EXECUTE format(
      'ALTER TABLE straylight.%I FORCE ROW LEVEL SECURITY',
      table_name
    );
    EXECUTE format(
      'CREATE POLICY simple_user_select ON straylight.%I '
      'FOR SELECT TO app_rw, app_ro '
      'USING (straylight_auth.can_access_user(user_id))',
      table_name
    );
    EXECUTE format(
      'CREATE POLICY simple_user_write ON straylight.%I '
      'FOR ALL TO app_rw '
      'USING ('
      '  straylight_auth.can_access_user(user_id) '
      '  AND straylight_auth.has_any_capability('
      '    ARRAY[''save'', ''checkpoint'', ''stage'', ''dream'', ''delete'']'
      '  )'
      ') '
      'WITH CHECK ('
      '  straylight_auth.can_access_user(user_id) '
      '  AND straylight_auth.has_any_capability('
      '    ARRAY[''save'', ''checkpoint'', ''stage'', ''dream'', ''delete'']'
      '  )'
      ')',
      table_name
    );
  END LOOP;
END;
$$;

GRANT SELECT, INSERT, UPDATE, DELETE ON
  straylight.briefing_stories,
  straylight.briefing_story_urls
TO app_rw;

GRANT SELECT ON
  straylight.briefing_stories,
  straylight.briefing_story_urls
TO app_ro;
