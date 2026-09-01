-- Migration 0083 rewrote schema-qualified function bodies, but PostgreSQL
-- stores string literals in constraints, indexes, and row-level-security
-- policies independently. Existing databases also retain the retired
-- reserved workspace paths and checkpoint metadata key written by the old
-- binaries. Repair those active catalog/data contracts without changing
-- historical content or audit payloads.

CREATE TEMP TABLE brunn_wave2_constraint_rewrites (
  schema_name text NOT NULL,
  relation_name text NOT NULL,
  constraint_name text NOT NULL,
  definition text NOT NULL,
  PRIMARY KEY (schema_name, relation_name, constraint_name)
) ON COMMIT DROP;

CREATE TEMP TABLE brunn_wave2_index_rewrites (
  schema_name text NOT NULL,
  index_name text NOT NULL,
  definition text NOT NULL,
  PRIMARY KEY (schema_name, index_name)
) ON COMMIT DROP;

DO $brunn_catalog_and_reserved_paths$
DECLARE
  retired_product text := concat('stray', 'light');
  retired_title text := initcap(retired_product);
  retired_reserved_prefix text := concat('.', retired_product, '/');
  canonical_reserved_prefix constant text := '.brunn/';
  retired_hash_key text := concat('_', retired_product, '_idempotency_hash');
  canonical_hash_key constant text := '_brunn_idempotency_hash';
  retired_app_id text := concat('com.rourkem.', retired_product);
  catalog_row record;
  rewritten_using text;
  rewritten_check text;
BEGIN
  IF to_regnamespace('brunn') IS NULL
     OR to_regnamespace('brunn_auth') IS NULL
     OR to_regnamespace(retired_product) IS NOT NULL
     OR to_regnamespace(concat(retired_product, '_auth')) IS NOT NULL THEN
    RAISE EXCEPTION 'catalog repair requires the complete canonical schema pair'
      USING ERRCODE = '55000';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM brunn.entries AS source
    JOIN brunn.entries AS target
      ON target.user_id=source.user_id
     AND target.path=canonical_reserved_prefix
       || substring(
         source.path FROM char_length(retired_reserved_prefix) + 1
       )
     AND target.id<>source.id
    WHERE left(source.path, char_length(retired_reserved_prefix))
          = retired_reserved_prefix
  ) THEN
    RAISE EXCEPTION 'a canonical reserved entry path already conflicts with its retired path'
      USING ERRCODE = '23505';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM brunn.messaging_conversations AS source
    JOIN brunn.messaging_conversations AS target
      ON target.user_id=source.user_id
     AND target.path=canonical_reserved_prefix
       || substring(
         source.path FROM char_length(retired_reserved_prefix) + 1
       )
     AND target.conversation_id<>source.conversation_id
    WHERE left(source.path, char_length(retired_reserved_prefix))
          = retired_reserved_prefix
  ) THEN
    RAISE EXCEPTION 'a canonical conversation path already conflicts with its retired path'
      USING ERRCODE = '23505';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM brunn.entry_versions
    WHERE metadata ? retired_hash_key
      AND metadata ? canonical_hash_key
      AND metadata->retired_hash_key IS DISTINCT FROM metadata->canonical_hash_key
  ) THEN
    RAISE EXCEPTION 'checkpoint metadata contains conflicting retired and canonical hashes'
      USING ERRCODE = '23505';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM brunn.api_credentials AS source
    JOIN brunn.api_credentials AS target
      ON target.user_id=source.user_id
     AND target.id<>source.id
     AND target.label=replace(
       replace(source.label, retired_title, 'Brunn'),
       retired_product,
       'brunn'
     )
    WHERE position(retired_product IN lower(source.label)) > 0
  ) THEN
    RAISE EXCEPTION 'a canonical credential label already conflicts with its retired label'
      USING ERRCODE = '23505';
  END IF;

  INSERT INTO brunn_wave2_constraint_rewrites (
    schema_name,
    relation_name,
    constraint_name,
    definition
  )
  SELECT namespace.nspname,
         relation.relname,
         constraint_record.conname,
         replace(
           replace(
             pg_get_constraintdef(constraint_record.oid, true),
             retired_title,
             'Brunn'
           ),
           retired_product,
           'brunn'
         )
  FROM pg_catalog.pg_constraint AS constraint_record
  JOIN pg_catalog.pg_class AS relation
    ON relation.oid=constraint_record.conrelid
  JOIN pg_catalog.pg_namespace AS namespace
    ON namespace.oid=relation.relnamespace
  WHERE namespace.nspname IN ('brunn', 'brunn_auth')
    AND position(
      retired_product
      IN lower(pg_get_constraintdef(constraint_record.oid, true))
    ) > 0;

  FOR catalog_row IN
    SELECT schema_name, relation_name, constraint_name
    FROM brunn_wave2_constraint_rewrites
    ORDER BY schema_name, relation_name, constraint_name
  LOOP
    EXECUTE format(
      'ALTER TABLE %I.%I DROP CONSTRAINT %I',
      catalog_row.schema_name,
      catalog_row.relation_name,
      catalog_row.constraint_name
    );
  END LOOP;

  INSERT INTO brunn_wave2_index_rewrites (
    schema_name,
    index_name,
    definition
  )
  SELECT namespace.nspname,
         index_relation.relname,
         replace(
           replace(
             pg_get_indexdef(index_record.indexrelid),
             retired_title,
             'Brunn'
           ),
           retired_product,
           'brunn'
         )
  FROM pg_catalog.pg_index AS index_record
  JOIN pg_catalog.pg_class AS index_relation
    ON index_relation.oid=index_record.indexrelid
  JOIN pg_catalog.pg_namespace AS namespace
    ON namespace.oid=index_relation.relnamespace
  WHERE namespace.nspname IN ('brunn', 'brunn_auth')
    AND position(
      retired_product IN lower(pg_get_indexdef(index_record.indexrelid))
    ) > 0;

  FOR catalog_row IN
    SELECT schema_name, index_name
    FROM brunn_wave2_index_rewrites
    ORDER BY schema_name, index_name
  LOOP
    EXECUTE format(
      'DROP INDEX %I.%I',
      catalog_row.schema_name,
      catalog_row.index_name
    );
  END LOOP;

  UPDATE brunn.entries
  SET path=canonical_reserved_prefix
    || substring(path FROM char_length(retired_reserved_prefix) + 1)
  WHERE left(path, char_length(retired_reserved_prefix))
        = retired_reserved_prefix;

  UPDATE brunn.messaging_conversations
  SET path=canonical_reserved_prefix
    || substring(path FROM char_length(retired_reserved_prefix) + 1)
  WHERE left(path, char_length(retired_reserved_prefix))
        = retired_reserved_prefix;

  UPDATE brunn.search_chunks
  SET path=canonical_reserved_prefix
    || substring(path FROM char_length(retired_reserved_prefix) + 1)
  WHERE left(path, char_length(retired_reserved_prefix))
        = retired_reserved_prefix;

  UPDATE brunn.workspace_changes
  SET path=canonical_reserved_prefix
    || substring(path FROM char_length(retired_reserved_prefix) + 1)
  WHERE left(path, char_length(retired_reserved_prefix))
        = retired_reserved_prefix;

  UPDATE brunn.entry_versions
  SET metadata=replace(
    metadata::text,
    retired_reserved_prefix,
    canonical_reserved_prefix
  )::jsonb
  WHERE position(retired_reserved_prefix IN metadata::text) > 0;

  UPDATE brunn.entry_versions
  SET metadata=jsonb_set(
    metadata - retired_hash_key,
    ARRAY[canonical_hash_key],
    metadata->retired_hash_key,
    true
  )
  WHERE metadata ? retired_hash_key;

  UPDATE brunn.api_credentials
  SET label=replace(
    replace(label, retired_title, 'Brunn'),
    retired_product,
    'brunn'
  )
  WHERE position(retired_product IN lower(label)) > 0;

  UPDATE brunn.notification_installations
  SET app_id='com.rourkem.brunn',
      updated_at=clock_timestamp()
  WHERE app_id=retired_app_id;

  FOR catalog_row IN
    SELECT schema_name, relation_name, constraint_name, definition
    FROM brunn_wave2_constraint_rewrites
    ORDER BY schema_name, relation_name, constraint_name
  LOOP
    EXECUTE format(
      'ALTER TABLE %I.%I ADD CONSTRAINT %I %s',
      catalog_row.schema_name,
      catalog_row.relation_name,
      catalog_row.constraint_name,
      catalog_row.definition
    );
  END LOOP;

  FOR catalog_row IN
    SELECT schema_name, index_name, definition
    FROM brunn_wave2_index_rewrites
    ORDER BY schema_name, index_name
  LOOP
    EXECUTE catalog_row.definition;
  END LOOP;

  FOR catalog_row IN
    SELECT namespace.nspname AS schema_name,
           relation.relname AS relation_name,
           policy.polname AS policy_name,
           pg_get_expr(policy.polqual, policy.polrelid, true) AS using_expression,
           pg_get_expr(policy.polwithcheck, policy.polrelid, true) AS check_expression
    FROM pg_catalog.pg_policy AS policy
    JOIN pg_catalog.pg_class AS relation ON relation.oid=policy.polrelid
    JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid=relation.relnamespace
    WHERE namespace.nspname IN ('brunn', 'brunn_auth')
      AND position(
        retired_product IN lower(
          coalesce(pg_get_expr(policy.polqual, policy.polrelid, true), '')
          || ' '
          || coalesce(pg_get_expr(policy.polwithcheck, policy.polrelid, true), '')
        )
      ) > 0
    ORDER BY namespace.nspname, relation.relname, policy.polname
  LOOP
    rewritten_using := CASE
      WHEN catalog_row.using_expression IS NULL THEN ''
      ELSE format(
        ' USING (%s)',
        replace(
          replace(catalog_row.using_expression, retired_title, 'Brunn'),
          retired_product,
          'brunn'
        )
      )
    END;
    rewritten_check := CASE
      WHEN catalog_row.check_expression IS NULL THEN ''
      ELSE format(
        ' WITH CHECK (%s)',
        replace(
          replace(catalog_row.check_expression, retired_title, 'Brunn'),
          retired_product,
          'brunn'
        )
      )
    END;
    EXECUTE format(
      'ALTER POLICY %I ON %I.%I%s%s',
      catalog_row.policy_name,
      catalog_row.schema_name,
      catalog_row.relation_name,
      rewritten_using,
      rewritten_check
    );
  END LOOP;

  IF EXISTS (
    SELECT 1
    FROM pg_catalog.pg_constraint AS constraint_record
    JOIN pg_catalog.pg_class AS relation
      ON relation.oid=constraint_record.conrelid
    JOIN pg_catalog.pg_namespace AS namespace
      ON namespace.oid=relation.relnamespace
    WHERE namespace.nspname IN ('brunn', 'brunn_auth')
      AND position(
        retired_product
        IN lower(pg_get_constraintdef(constraint_record.oid, true))
      ) > 0
  ) OR EXISTS (
    SELECT 1
    FROM pg_catalog.pg_index AS index_record
    JOIN pg_catalog.pg_class AS index_relation
      ON index_relation.oid=index_record.indexrelid
    JOIN pg_catalog.pg_namespace AS namespace
      ON namespace.oid=index_relation.relnamespace
    WHERE namespace.nspname IN ('brunn', 'brunn_auth')
      AND position(
        retired_product IN lower(pg_get_indexdef(index_record.indexrelid))
      ) > 0
  ) OR EXISTS (
    SELECT 1
    FROM pg_catalog.pg_policy AS policy
    JOIN pg_catalog.pg_class AS relation ON relation.oid=policy.polrelid
    JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid=relation.relnamespace
    WHERE namespace.nspname IN ('brunn', 'brunn_auth')
      AND position(
        retired_product IN lower(
          coalesce(pg_get_expr(policy.polqual, policy.polrelid, true), '')
          || ' '
          || coalesce(pg_get_expr(policy.polwithcheck, policy.polrelid, true), '')
        )
      ) > 0
  ) THEN
    RAISE EXCEPTION 'a catalog expression retained the retired product identity'
      USING ERRCODE = '55000';
  END IF;

  IF EXISTS (
    SELECT 1 FROM brunn.entries
    WHERE left(path, char_length(retired_reserved_prefix))=retired_reserved_prefix
  ) OR EXISTS (
    SELECT 1 FROM brunn.messaging_conversations
    WHERE left(path, char_length(retired_reserved_prefix))=retired_reserved_prefix
  ) OR EXISTS (
    SELECT 1 FROM brunn.search_chunks
    WHERE left(path, char_length(retired_reserved_prefix))=retired_reserved_prefix
  ) OR EXISTS (
    SELECT 1 FROM brunn.workspace_changes
    WHERE left(path, char_length(retired_reserved_prefix))=retired_reserved_prefix
  ) OR EXISTS (
    SELECT 1 FROM brunn.entry_versions
    WHERE metadata ? retired_hash_key
       OR position(retired_reserved_prefix IN metadata::text) > 0
  ) OR EXISTS (
    SELECT 1 FROM brunn.notification_installations
    WHERE app_id=retired_app_id
  ) THEN
    RAISE EXCEPTION 'active data retained a retired reserved identity'
      USING ERRCODE = '55000';
  END IF;
END
$brunn_catalog_and_reserved_paths$;
