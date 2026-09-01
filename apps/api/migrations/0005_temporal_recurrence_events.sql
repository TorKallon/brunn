CREATE TABLE brunn.iana_time_zones (
  name text PRIMARY KEY
);

INSERT INTO brunn.iana_time_zones (name)
SELECT DISTINCT name FROM pg_timezone_names;

CREATE TABLE brunn.temporal_specs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  schema_version integer NOT NULL DEFAULT 1 CHECK (schema_version = 1),
  kind text NOT NULL CHECK (kind IN (
    'date', 'date_interval', 'local_datetime', 'floating_datetime',
    'instant', 'instant_interval', 'local_interval', 'due'
  )),
  date_value date,
  start_date date,
  end_date_exclusive date,
  local_value timestamp without time zone,
  instant_value timestamptz,
  start_instant timestamptz,
  end_instant timestamptz,
  start_local timestamp without time zone,
  end_local timestamp without time zone,
  time_zone text REFERENCES brunn.iana_time_zones(name),
  duration interval,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  CHECK (duration IS NULL OR duration >= interval '0 seconds'),
  CHECK (
    (kind = 'date'
      AND date_value IS NOT NULL
      AND start_date IS NULL AND end_date_exclusive IS NULL
      AND local_value IS NULL AND instant_value IS NULL
      AND start_instant IS NULL AND end_instant IS NULL
      AND start_local IS NULL AND end_local IS NULL AND time_zone IS NULL)
    OR
    (kind = 'date_interval'
      AND start_date IS NOT NULL
      AND (end_date_exclusive IS NULL OR end_date_exclusive > start_date)
      AND date_value IS NULL AND local_value IS NULL AND instant_value IS NULL
      AND start_instant IS NULL AND end_instant IS NULL
      AND start_local IS NULL AND end_local IS NULL AND time_zone IS NULL)
    OR
    (kind IN ('local_datetime', 'due')
      AND local_value IS NOT NULL AND time_zone IS NOT NULL
      AND date_value IS NULL AND start_date IS NULL AND end_date_exclusive IS NULL
      AND instant_value IS NULL AND start_instant IS NULL AND end_instant IS NULL
      AND start_local IS NULL AND end_local IS NULL)
    OR
    (kind = 'floating_datetime'
      AND local_value IS NOT NULL AND time_zone IS NULL
      AND date_value IS NULL AND start_date IS NULL AND end_date_exclusive IS NULL
      AND instant_value IS NULL AND start_instant IS NULL AND end_instant IS NULL
      AND start_local IS NULL AND end_local IS NULL)
    OR
    (kind = 'instant'
      AND instant_value IS NOT NULL
      AND date_value IS NULL AND start_date IS NULL AND end_date_exclusive IS NULL
      AND local_value IS NULL AND start_instant IS NULL AND end_instant IS NULL
      AND start_local IS NULL AND end_local IS NULL AND time_zone IS NULL)
    OR
    (kind = 'instant_interval'
      AND start_instant IS NOT NULL
      AND (end_instant IS NULL OR end_instant > start_instant)
      AND date_value IS NULL AND start_date IS NULL AND end_date_exclusive IS NULL
      AND local_value IS NULL AND instant_value IS NULL
      AND start_local IS NULL AND end_local IS NULL AND time_zone IS NULL)
    OR
    (kind = 'local_interval'
      AND start_local IS NOT NULL AND time_zone IS NOT NULL
      AND (end_local IS NULL OR end_local > start_local)
      AND date_value IS NULL AND start_date IS NULL AND end_date_exclusive IS NULL
      AND local_value IS NULL AND instant_value IS NULL
      AND start_instant IS NULL AND end_instant IS NULL)
  )
);

CREATE TRIGGER temporal_specs_register_record
BEFORE INSERT ON brunn.temporal_specs
FOR EACH ROW EXECUTE FUNCTION brunn.register_record_key('temporal_spec');

CREATE TABLE brunn.recurrence_specs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  schema_version integer NOT NULL DEFAULT 1 CHECK (schema_version = 1),
  start_temporal_id uuid NOT NULL,
  duration interval CHECK (duration IS NULL OR duration >= interval '0 seconds'),
  split_from_series_object_id uuid,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, start_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, split_from_series_object_id)
    REFERENCES brunn.objects(user_id, id)
);

CREATE TRIGGER recurrence_specs_register_record
BEFORE INSERT ON brunn.recurrence_specs
FOR EACH ROW EXECUTE FUNCTION brunn.register_record_key('recurrence_spec');

CREATE TABLE brunn.recurrence_rules (
  user_id uuid NOT NULL,
  recurrence_spec_id uuid NOT NULL,
  rule_order integer NOT NULL CHECK (rule_order >= 0),
  rrule brunn.nonempty_text NOT NULL CHECK (rrule ~ '^FREQ='),
  PRIMARY KEY (user_id, recurrence_spec_id, rule_order),
  FOREIGN KEY (user_id, recurrence_spec_id)
    REFERENCES brunn.recurrence_specs(user_id, id)
);

CREATE TABLE brunn.recurrence_exclusions (
  user_id uuid NOT NULL,
  recurrence_spec_id uuid NOT NULL,
  recurrence_id_temporal_id uuid NOT NULL,
  evidence_id uuid NOT NULL,
  PRIMARY KEY (user_id, recurrence_spec_id, recurrence_id_temporal_id),
  FOREIGN KEY (user_id, recurrence_spec_id)
    REFERENCES brunn.recurrence_specs(user_id, id),
  FOREIGN KEY (user_id, recurrence_id_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES brunn.evidence_items(user_id, id)
);

CREATE TABLE brunn.event_series_revisions (
  user_id uuid NOT NULL,
  object_id uuid NOT NULL,
  object_version integer NOT NULL,
  source_uid brunn.nonempty_text NOT NULL,
  recurrence_spec_id uuid,
  original_temporal_id uuid NOT NULL,
  current_temporal_id uuid NOT NULL,
  scheduling_sequence integer,
  PRIMARY KEY (user_id, object_id, object_version),
  FOREIGN KEY (user_id, object_id, object_version)
    REFERENCES brunn.object_revisions(user_id, object_id, version),
  FOREIGN KEY (user_id, recurrence_spec_id)
    REFERENCES brunn.recurrence_specs(user_id, id),
  FOREIGN KEY (user_id, original_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, current_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id)
);

CREATE TABLE brunn.event_occurrences (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  occurrence_object_id uuid NOT NULL,
  series_object_id uuid NOT NULL,
  canonical_recurrence_id brunn.nonempty_text NOT NULL,
  recurrence_id_temporal_id uuid NOT NULL,
  original_temporal_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, occurrence_object_id),
  UNIQUE (user_id, series_object_id, canonical_recurrence_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, occurrence_object_id)
    REFERENCES brunn.objects(user_id, id),
  FOREIGN KEY (user_id, series_object_id)
    REFERENCES brunn.objects(user_id, id),
  FOREIGN KEY (user_id, recurrence_id_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, original_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id)
);

CREATE TABLE brunn.event_occurrence_revisions (
  user_id uuid NOT NULL,
  occurrence_object_id uuid NOT NULL,
  object_version integer NOT NULL,
  current_temporal_id uuid NOT NULL,
  actual_temporal_id uuid,
  exception_kind text NOT NULL DEFAULT 'generated' CHECK (
    exception_kind IN ('generated', 'moved', 'cancelled', 'excluded', 'added')
  ),
  evidence_id uuid NOT NULL,
  PRIMARY KEY (user_id, occurrence_object_id, object_version),
  FOREIGN KEY (user_id, occurrence_object_id)
    REFERENCES brunn.event_occurrences(user_id, occurrence_object_id),
  FOREIGN KEY (user_id, occurrence_object_id, object_version)
    REFERENCES brunn.object_revisions(user_id, object_id, version),
  FOREIGN KEY (user_id, current_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, actual_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES brunn.evidence_items(user_id, id)
);

CREATE TABLE brunn.recurrence_additions (
  user_id uuid NOT NULL,
  recurrence_spec_id uuid NOT NULL,
  occurrence_object_id uuid NOT NULL,
  evidence_id uuid NOT NULL,
  PRIMARY KEY (user_id, recurrence_spec_id, occurrence_object_id),
  FOREIGN KEY (user_id, recurrence_spec_id)
    REFERENCES brunn.recurrence_specs(user_id, id),
  FOREIGN KEY (user_id, occurrence_object_id)
    REFERENCES brunn.event_occurrences(user_id, occurrence_object_id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES brunn.evidence_items(user_id, id)
);

CREATE TRIGGER temporal_specs_immutable
BEFORE UPDATE OR DELETE ON brunn.temporal_specs
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER recurrence_specs_immutable
BEFORE UPDATE OR DELETE ON brunn.recurrence_specs
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER recurrence_rules_immutable
BEFORE UPDATE OR DELETE ON brunn.recurrence_rules
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER recurrence_exclusions_immutable
BEFORE UPDATE OR DELETE ON brunn.recurrence_exclusions
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER recurrence_additions_immutable
BEFORE UPDATE OR DELETE ON brunn.recurrence_additions
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER event_series_revisions_immutable
BEFORE UPDATE OR DELETE ON brunn.event_series_revisions
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER event_occurrences_immutable
BEFORE UPDATE OR DELETE ON brunn.event_occurrences
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER event_occurrence_revisions_immutable
BEFORE UPDATE OR DELETE ON brunn.event_occurrence_revisions
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

