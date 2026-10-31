-- Privileges for engineers who sign in to Hydra with their own Postgres role.
--
-- Run once per database, after `hydra-admin migrate`, as the role that runs migrations, which
-- must be a superuser (the script refuses to run otherwise, and changes nothing):
--   psql "postgres://admin@db:5433/hydra" -f ops/roles.sql
-- Then, as that superuser, add the first administrator, who adds everyone else in Hydra's
-- Account tab (Hydra, hydra-mcp and revisions record these names):
--   CREATE ROLE boss LOGIN PASSWORD '...' IN ROLE hydra_admin;
-- and run this script again, so other administrators can manage boss too.
-- Safe to run again; run it again after adding users by hand (CREATE ROLE ... IN ROLE
-- hydra_engineer, or hydra_admin) so administrators can manage them.
--
-- Triggers, not grants, keep catalogue rows, revisions, reviewed proposals and stored files
-- from being edited, and record who did what; these grants only keep engineers away from the
-- schema and the migration history.

-- All or nothing, even when psql carries on after an error.
BEGIN;

DO $$
BEGIN
    IF NOT (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) THEN
        RAISE EXCEPTION 'ops/roles.sql must run as a database superuser; % is not one', current_user;
    END IF;
END
$$;

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

-- Hydra keeps no large objects, and objects a user owns keep administrators from changing or
-- deleting them.
REVOKE EXECUTE ON FUNCTION
    pg_catalog.lo_create(oid), pg_catalog.lo_creat(integer), pg_catalog.lo_from_bytea(oid, bytea),
    pg_catalog.lo_import(text), pg_catalog.lo_import(text, oid)
FROM PUBLIC;

-- Administrators are members of hydra_admin, which has no rights beyond hydra_engineer's.
-- They manage users only through migration 0006's functions, which run as hydra_manager: it
-- can create roles, end sessions and see everyone's, and nobody signs in as it. It owns their
-- schema hydra_auth and everything in it, so nobody else can change them.
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'hydra_admin') THEN
        CREATE ROLE hydra_admin NOLOGIN;
    END IF;
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'hydra_manager') THEN
        CREATE ROLE hydra_manager NOLOGIN;
    END IF;
    -- 16+ can keep hydra_manager from using the groups' rights; it only needs to grant them.
    IF current_setting('server_version_num')::int >= 160000 THEN
        EXECUTE 'GRANT hydra_engineer, hydra_admin TO hydra_manager WITH ADMIN TRUE, INHERIT FALSE, SET FALSE';
    ELSE
        GRANT hydra_engineer, hydra_admin TO hydra_manager WITH ADMIN OPTION;
    END IF;
END
$$;
ALTER ROLE hydra_manager NOLOGIN CREATEROLE;
GRANT hydra_engineer TO hydra_admin;
GRANT pg_signal_backend, pg_read_all_stats TO hydra_manager;

ALTER SCHEMA hydra_auth OWNER TO hydra_manager;
REVOKE ALL ON SCHEMA hydra_auth FROM PUBLIC;
GRANT USAGE ON SCHEMA hydra_auth TO hydra_engineer;
DO $$
DECLARE
    f regprocedure;
BEGIN
    FOR f IN SELECT p.oid FROM pg_proc p WHERE p.pronamespace = 'hydra_auth'::regnamespace LOOP
        EXECUTE format('ALTER FUNCTION %s OWNER TO hydra_manager', f);
    END LOOP;
END
$$;
GRANT EXECUTE ON FUNCTION
    hydra_auth.hydra_list_users(), hydra_auth.hydra_create_user(text, text, boolean),
    hydra_auth.hydra_set_password(text, text), hydra_auth.hydra_set_admin(text, boolean),
    hydra_auth.hydra_end_sessions(text), hydra_auth.hydra_set_can_login(text, boolean),
    hydra_auth.hydra_delete_user(text), hydra_auth.hydra_end_deleted_sessions(oid)
TO hydra_admin;

-- Nobody but hydra_manager holds ADMIN OPTION on hydra_engineer, hydra_admin or a user, nobody
-- is a member of a user, and hydra_admin is a member of hydra_engineer only. Earlier versions
-- of this script made administrators CREATEROLE roles that managed users through such grants;
-- on PostgreSQL 15 that is as good as superuser, so no Hydra user keeps CREATEROLE.
-- PostgreSQL 16+ keeps one grant per grantor and hydra_manager can only take back its own, so
-- it grants the groups' memberships again, before the others' grants go.
-- Then it gets ADMIN OPTION, which it needs on 16+, on every real user: a login role with no
-- rights or memberships beyond a Hydra user's. Roles that are anything more stay with the
-- superuser, so administrators can never take over their rights (and the functions give no
-- rights to users who own objects). On 16+ it may use none of the users' rights.
DO $$
DECLARE
    pg16 boolean := current_setting('server_version_num')::int >= 160000;
    engineer oid := 'hydra_engineer'::regrole;
    admin oid := 'hydra_admin'::regrole;
    manager oid := 'hydra_manager'::regrole;
    groups oid[] := ARRAY[engineer, admin];
    members oid[];
    r record;
BEGIN
    FOR r IN
        SELECT u.rolname FROM pg_roles u
        WHERE u.rolcreaterole AND NOT u.rolsuper AND u.oid <> manager
          AND (pg_has_role(u.oid, engineer, 'MEMBER') OR pg_has_role(u.oid, admin, 'MEMBER'))
    LOOP
        RAISE WARNING 'took CREATEROLE from %: Hydra users never need it', r.rolname;
        EXECUTE format('ALTER ROLE %I NOCREATEROLE', r.rolname);
    END LOOP;

    SELECT coalesce(array_agg(DISTINCT a.member), '{}') INTO members FROM pg_auth_members a
    WHERE a.roleid = ANY (groups) AND a.member NOT IN (admin, manager);

    IF pg16 THEN
        FOR r IN
            SELECT DISTINCT a.roleid::regrole AS role, a.member::regrole AS member
            FROM pg_auth_members a
            WHERE a.roleid = ANY (groups) AND a.member = ANY (members) AND a.grantor <> manager
              AND NOT EXISTS (SELECT FROM pg_auth_members b WHERE b.roleid = a.roleid
                                AND b.member = a.member AND b.grantor = manager)
        LOOP
            EXECUTE format('GRANT %s TO %s GRANTED BY hydra_manager', r.role, r.member);
        END LOOP;
    END IF;

    FOR r IN
        SELECT a.roleid, a.member, a.grantor,
               a.roleid = ANY (groups)
                   AND NOT (pg16 AND a.member <> admin AND a.grantor <> manager) AS admin_only
        FROM pg_auth_members a
        WHERE a.roleid = ANY (groups)
                AND (a.member = ANY (members) AND pg16 AND a.grantor <> manager
                     OR a.member = ANY (members || admin) AND a.admin_option)
           OR a.member = admin AND a.roleid <> engineer
           OR a.roleid = ANY (members) AND a.member = ANY (members)
    LOOP
        -- CASCADE takes grants made with a revoked ADMIN OPTION along: skip those.
        CONTINUE WHEN NOT EXISTS (SELECT FROM pg_auth_members a WHERE a.roleid = r.roleid
                                    AND a.member = r.member AND a.grantor = r.grantor);
        EXECUTE format('REVOKE %s%s FROM %s%s CASCADE',
                       CASE WHEN r.admin_only THEN 'ADMIN OPTION FOR ' END, r.roleid::regrole,
                       r.member::regrole,
                       CASE WHEN pg16 THEN ' GRANTED BY ' || r.grantor::regrole END);
    END LOOP;

    FOR r IN
        SELECT u.oid::regrole AS member FROM pg_roles u
        WHERE u.oid = ANY (members) AND u.rolcanlogin AND u.rolname !~ '^pg_'
          AND NOT (u.rolsuper OR u.rolcreaterole OR u.rolcreatedb OR u.rolreplication OR u.rolbypassrls)
          AND NOT EXISTS (SELECT FROM pg_auth_members a
                          WHERE a.member = u.oid AND a.roleid <> ALL (groups))
          AND NOT EXISTS (SELECT FROM pg_auth_members a
                          WHERE a.roleid = u.oid AND a.member = manager AND a.admin_option)
    LOOP
        IF pg16 THEN
            EXECUTE format('GRANT %s TO hydra_manager WITH ADMIN TRUE, INHERIT FALSE, SET FALSE', r.member);
        ELSE
            EXECUTE format('GRANT %s TO hydra_manager WITH ADMIN OPTION', r.member);
        END IF;
    END LOOP;

    IF pg16 THEN
        FOR r IN
            SELECT a.roleid::regrole AS role, a.grantor::regrole AS grantor FROM pg_auth_members a
            WHERE a.member = manager AND a.roleid = ANY (members || groups)
              AND (a.inherit_option OR a.set_option)
        LOOP
            EXECUTE format('GRANT %s TO hydra_manager WITH INHERIT FALSE, SET FALSE GRANTED BY %s',
                           r.role, r.grantor);
        END LOOP;
    END IF;
END
$$;

COMMIT;
