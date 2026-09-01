CREATE OR REPLACE FUNCTION brunn.lexical_source_text(p_source_ref text)
RETURNS text
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
STRICT
AS $$
  SELECT regexp_replace(
    regexp_replace(p_source_ref, '([a-z0-9])([A-Z])', '\1 \2', 'g'),
    '[^[:alnum:]]+',
    ' ',
    'g'
  )
$$;

COMMENT ON FUNCTION brunn.lexical_source_text(text) IS
  'Normalizes path separators and camel-cased segments for lexical source-path matching.';
