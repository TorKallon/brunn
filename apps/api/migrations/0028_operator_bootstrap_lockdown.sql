-- Bootstrap is a deployment/operator primitive, not an application write API.
-- Development bootstrap and the isolated evaluation importer use the admin
-- pool, so app_rw does not need this authority.
REVOKE EXECUTE ON FUNCTION straylight_auth.bootstrap_user(
  text, text, text, text, text[]
) FROM app_rw, app_ro;
