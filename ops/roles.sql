-- Privileges for engineers who sign in to Hydra with their own Postgres role.
--
-- Run once per database, after `hydra-admin migrate`, as the role that runs migrations:
--   psql "postgres://admin@db:5433/hydra" -f ops/roles.sql
-- Then add each engineer (Hydra, hydra-mcp and revisions record this name):
--   CREATE ROLE alice LOGIN PASSWORD '...' IN ROLE hydra_engineer;
-- Safe to run again.
--
-- Triggers, not grants, keep catalogue rows, revisions, reviewed proposals and stored files
-- from being edited, and record who did what; these grants only keep engineers away from the
-- schema and the migration history.

DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'hydra_engineer') THEN
        CREATE ROLE hydra_engineer NOLOGIN;
    END IF;
END
$$;

GRANT USAGE ON SCHEMA public TO hydra_engineer;
-- PostgreSQL 15 already does this for new clusters, not for ones upgraded from 14 or older.
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
-- UPDATE also covers the app's SELECT ... FOR UPDATE and LOCK TABLE; DELETE is only needed
-- where a valve type's models are replaced.
GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA public TO hydra_engineer;
GRANT DELETE ON part_assets TO hydra_engineer;
REVOKE INSERT, UPDATE ON _sqlx_migrations FROM hydra_engineer;
REVOKE UPDATE ON assets FROM hydra_engineer;

-- Tables later migrations add, when this same role runs them.
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE ON TABLES TO hydra_engineer;
