-- A single derived snapshot separates usable positional evidence from the
-- latest report (which may be coarse) and delayed historical visit state.
ALTER TABLE brunn.location_presence ADD COLUMN current_position jsonb;
ALTER TABLE brunn.location_presence ADD CONSTRAINT location_current_position_object
  CHECK (current_position IS NULL OR jsonb_typeof(current_position) = 'object');

-- Preserve retained evidence at upgrade without treating callback delivery
-- time as a fresh observation. Known-place resolution is deliberately left to
-- the application fold/rederive; an old open visit is not current-place proof.
WITH candidates AS (
  SELECT report.*,
         CASE report.type
           WHEN 'visit_arrival' THEN LEAST(COALESCE(report.arrived_at, report.at), report.at)
           WHEN 'visit_departure' THEN LEAST(report.departed_at, report.at)
           ELSE report.at
         END AS observed_at
  FROM brunn.location_reports AS report
  WHERE report.accuracy_m <= 1000
), latest AS (
  SELECT DISTINCT ON (user_id) *
  FROM candidates
  ORDER BY user_id, observed_at DESC, accuracy_m, type, at
)
UPDATE brunn.location_presence AS presence
SET current_position = jsonb_build_object(
  'observed_at', latest.observed_at,
  'source_type', latest.type, 'source_reported_at', latest.at,
  'coordinate', jsonb_build_object('lat', latest.lat, 'lon', latest.lon),
  'accuracy_m', latest.accuracy_m,
  'timezone', presence.timezone,
  'city', latest.city, 'region', latest.region, 'country', latest.country,
  'place', CASE WHEN latest.accuracy_m <= 200 AND latest.name IS NOT NULL
    THEN jsonb_build_object('label', latest.name, 'kind', 'unknown', 'confidence', 'low')
    ELSE NULL END
)
FROM latest WHERE presence.user_id = latest.user_id;

-- If raw retention has elapsed, retain the existing usable coordinate only.
-- Its original age remains visible, and no visit label is inferred.
UPDATE brunn.location_presence
SET current_position = jsonb_build_object(
  'observed_at', reported_at,
  'source_type', 'unknown', 'source_reported_at', reported_at,
  'coordinate', jsonb_build_object('lat', last_lat, 'lon', last_lon),
  'accuracy_m', last_accuracy_m, 'timezone', timezone,
  'city', city, 'region', region, 'country', country, 'place', NULL
)
WHERE current_position IS NULL AND last_accuracy_m <= 1000;
