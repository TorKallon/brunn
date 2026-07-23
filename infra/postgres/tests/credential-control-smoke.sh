#!/bin/sh
set -eu

: "${PGHOST:=db}"
: "${PGPORT:=5432}"
: "${PGDATABASE:=straylight_schema_validation}"
: "${APP_RW_PASSWORD:?set APP_RW_PASSWORD}"
: "${APP_RO_PASSWORD:?set APP_RO_PASSWORD}"

psql_rw() {
  PGPASSWORD="$APP_RW_PASSWORD" psql \
    -X -q -v ON_ERROR_STOP=1 \
    -h "$PGHOST" -p "$PGPORT" -U app_rw -d "$PGDATABASE" "$@"
}

psql_ro() {
  PGPASSWORD="$APP_RO_PASSWORD" psql \
    -X -q -v ON_ERROR_STOP=1 \
    -h "$PGHOST" -p "$PGPORT" -U app_ro -d "$PGDATABASE" "$@"
}

token_a=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
token_b=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
token_c=cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
token_d=dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd

psql_rw >/dev/null <<SQL
SELECT * FROM straylight_auth.bootstrap_user(
  'smoke:list:a', 'Smoke list A', 'reader-a', '$token_a',
  ARRAY['status', 'read']
);
SELECT * FROM straylight_auth.bootstrap_user(
  'smoke:list:b', 'Smoke list B', 'reader-b', '$token_b',
  ARRAY['status', 'read']
);
SELECT * FROM straylight_auth.bootstrap_user(
  'smoke:list:c', 'Smoke list C', 'controller-c', '$token_c',
  ARRAY['status', 'read', 'save']
);
SQL

psql_ro >/dev/null <<SQL
DO \$smoke\$
DECLARE
  authenticated record;
BEGIN
  SELECT * INTO STRICT authenticated
  FROM straylight_auth.authenticate_credential('$token_a');

  PERFORM set_config('app.current_user_id', authenticated.user_id::text, false);
  PERFORM set_config(
    'app.current_credential_id', authenticated.credential_id::text, false
  );
  PERFORM set_config(
    'app.capabilities', array_to_string(authenticated.capabilities, ','), false
  );
  PERFORM set_config(
    'app.scope_refs', array_to_string(authenticated.scope_refs, ','), false
  );

  IF (SELECT count(*) FROM straylight_auth.list_credentials(authenticated.user_id)) <> 1 THEN
    RAISE EXCEPTION 'app_ro did not receive exactly its own credential';
  END IF;

  IF NOT EXISTS (
    SELECT 1
    FROM straylight_auth.list_credentials(authenticated.user_id) AS credential
    WHERE credential.label = 'reader-a'
      AND credential.scope_refs = ARRAY['scope:root']
      AND credential.disabled_at IS NULL
  ) THEN
    RAISE EXCEPTION 'app_ro credential metadata projection is incomplete';
  END IF;

  IF has_function_privilege(
    current_user,
    'straylight_auth.issue_credential(uuid,text,text,text[],text[])',
    'EXECUTE'
  ) OR has_function_privilege(
    current_user,
    'straylight_auth.revoke_credential(uuid,uuid)',
    'EXECUTE'
  ) THEN
    RAISE EXCEPTION 'app_ro unexpectedly has issue or revoke execution rights';
  END IF;
END
\$smoke\$;
SQL

psql_rw >/dev/null <<SQL
DO \$smoke\$
DECLARE
  authenticated record;
  issued_id uuid;
BEGIN
  SELECT * INTO STRICT authenticated
  FROM straylight_auth.authenticate_credential('$token_c');

  PERFORM set_config('app.current_user_id', authenticated.user_id::text, false);
  PERFORM set_config(
    'app.current_credential_id', authenticated.credential_id::text, false
  );
  PERFORM set_config(
    'app.capabilities', array_to_string(authenticated.capabilities, ','), false
  );
  PERFORM set_config(
    'app.scope_refs', array_to_string(authenticated.scope_refs, ','), false
  );

  issued_id := straylight_auth.issue_credential(
    authenticated.user_id,
    'reader-c',
    '$token_d',
    ARRAY['status', 'read'],
    ARRAY['scope:root']
  );
  PERFORM straylight_auth.revoke_credential(authenticated.user_id, issued_id);

  IF (SELECT count(*) FROM straylight_auth.list_credentials(authenticated.user_id)) <> 2 THEN
    RAISE EXCEPTION 'app_rw did not receive both credential metadata rows';
  END IF;

  IF NOT EXISTS (
    SELECT 1
    FROM straylight_auth.list_credentials(authenticated.user_id) AS credential
    WHERE credential.id = issued_id
      AND credential.label = 'reader-c'
      AND credential.disabled_at IS NOT NULL
  ) THEN
    RAISE EXCEPTION 'revoked credential metadata was not listed safely';
  END IF;
END
\$smoke\$;
SQL

if psql_rw -c 'SELECT token_hash FROM straylight.api_credentials LIMIT 1' \
  >/dev/null 2>&1; then
  echo 'app_rw unexpectedly read api_credentials.token_hash' >&2
  exit 1
fi

if psql_ro -c 'SELECT credential_id FROM straylight.credential_scope_grants LIMIT 1' \
  >/dev/null 2>&1; then
  echo 'app_ro unexpectedly read credential_scope_grants' >&2
  exit 1
fi

if psql_ro >/dev/null 2>&1 <<SQL
SELECT *
FROM straylight_auth.list_credentials(
  (SELECT user_id FROM straylight_auth.authenticate_credential('$token_a'))
);
SQL
then
  echo 'list_credentials accepted an unset authentication context' >&2
  exit 1
fi

if psql_ro >/dev/null 2>&1 <<SQL
SELECT set_config('app.current_user_id', auth.user_id::text, false),
       set_config('app.current_credential_id', auth.credential_id::text, false),
       set_config('app.capabilities', array_to_string(auth.capabilities, ','), false),
       set_config('app.scope_refs', array_to_string(auth.scope_refs, ','), false)
FROM straylight_auth.authenticate_credential('$token_a') AS auth;
SELECT *
FROM straylight_auth.list_credentials(
  (SELECT user_id FROM straylight_auth.authenticate_credential('$token_b'))
);
SQL
then
  echo 'list_credentials accepted a cross-user request' >&2
  exit 1
fi

if psql_ro >/dev/null 2>&1 <<SQL
SELECT set_config('app.current_user_id', auth_a.user_id::text, false),
       set_config('app.current_credential_id', auth_b.credential_id::text, false),
       set_config('app.capabilities', array_to_string(auth_a.capabilities, ','), false),
       set_config('app.scope_refs', array_to_string(auth_a.scope_refs, ','), false)
FROM straylight_auth.authenticate_credential('$token_a') AS auth_a
CROSS JOIN straylight_auth.authenticate_credential('$token_b') AS auth_b;
SELECT *
FROM straylight_auth.list_credentials(
  (SELECT user_id FROM straylight_auth.authenticate_credential('$token_a'))
);
SQL
then
  echo 'list_credentials accepted a forged credential context' >&2
  exit 1
fi

if psql_rw >/dev/null 2>&1 <<SQL
SELECT set_config('app.current_user_id', auth.user_id::text, false),
       set_config('app.current_credential_id', auth.credential_id::text, false),
       set_config('app.capabilities', array_to_string(auth.capabilities, ','), false),
       set_config('app.scope_refs', array_to_string(auth.scope_refs, ','), false)
FROM straylight_auth.authenticate_credential('$token_a') AS auth;
SELECT straylight_auth.issue_credential(
  (SELECT user_id FROM straylight_auth.authenticate_credential('$token_a')),
  'must-fail',
  'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
  ARRAY['read'],
  ARRAY['scope:root']
);
SQL
then
  echo 'issue_credential accepted a context without save capability' >&2
  exit 1
fi

result_shape=$(psql_ro -At -c \
  "SELECT pg_get_function_result('straylight_auth.list_credentials(uuid)'::regprocedure)")
case "$result_shape" in
  *token_hash*)
    echo 'list_credentials exposes token_hash in its return shape' >&2
    exit 1
    ;;
esac

echo 'credential control smoke test passed'
