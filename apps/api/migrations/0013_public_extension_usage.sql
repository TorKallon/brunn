-- Extensions are installed in public, while application data remains isolated
-- in the Straylight schemas. App roles may resolve extension types/functions
-- but may never create objects in public.
REVOKE CREATE ON SCHEMA public FROM app_rw, app_ro;
GRANT USAGE ON SCHEMA public TO app_rw, app_ro;
