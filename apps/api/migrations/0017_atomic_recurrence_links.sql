ALTER TABLE straylight.event_series_revisions
  ALTER CONSTRAINT event_series_revisions_user_id_recurrence_spec_id_fkey
  DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE straylight.event_occurrences
  ALTER CONSTRAINT event_occurrences_user_id_series_object_id_fkey
  DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE straylight.recurrence_additions
  ALTER CONSTRAINT recurrence_additions_user_id_occurrence_object_id_fkey
  DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE straylight.recurrence_specs
  ALTER CONSTRAINT recurrence_specs_user_id_split_from_series_object_id_fkey
  DEFERRABLE INITIALLY DEFERRED;
