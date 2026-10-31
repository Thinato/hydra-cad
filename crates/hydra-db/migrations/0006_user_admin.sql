-- User administration for Hydra administrators (members of hydra_admin), who have no special
-- Postgres rights of their own: the functions below run as their owner, hydra_manager, which
-- ops/roles.sql creates, makes the owner of their schema hydra_auth and of everything in it,
-- and lets hydra_admin call them; until then only a superuser can. Nobody else can create,
-- change or drop anything in hydra_auth, not even a database owner who owns schema public, so
-- migrations that change it must run as a superuser. The signed-in role is session_user
-- (current_user is hydra_manager here). Refusals are plain words with SQLSTATE H0001.
-- Passwords only ever arrive as SCRAM verifiers, as bind parameters, so they never appear in
-- SQL text or logs; errors from statements that carry one are raised again without them.

-- "Who did it" comes from the signed-in role, not one it switched to with SET ROLE.
CREATE OR REPLACE FUNCTION stamp_circuit_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.owner := session_user;
    ELSE
        NEW.owner := OLD.owner;
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION stamp_revision_author() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.created_by := session_user;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION stamp_proposal_reviewer() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status = 'pending' AND NEW.status <> 'pending' THEN
        NEW.reviewed_by := session_user;
        NEW.reviewed_at := now();
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION start_proposal() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.status <> 'pending' OR NEW.reviewed_by IS NOT NULL OR NEW.reviewed_at IS NOT NULL
       OR NEW.review_note IS NOT NULL OR NEW.approved IS NOT NULL THEN
        RAISE EXCEPTION 'a new proposal starts pending; it is reviewed afterwards';
    END IF;
    NEW.created_by := session_user;
    RETURN NEW;
END
$$;

-- Not IF NOT EXISTS: one made by anyone else must not be used.
CREATE SCHEMA hydra_auth;

-- Refuses unless the signed-in role is an administrator and, when `username` isn't null, it
-- is a user name. `action` is what an administrator wants to do to that user: 'take' rights
-- away (disable, demote, end sessions), 'delete' them or 'give' rights (enable, promote, set
-- a password); null checks only the name. `refusal`, when not null, is why administrators
-- can't do this to themselves.
CREATE FUNCTION hydra_auth.hydra_user_check(username text, action text, refusal text)
RETURNS void LANGUAGE plpgsql STABLE SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    found pg_catalog.pg_roles;
BEGIN
    IF NOT pg_catalog.pg_has_role(session_user, 'hydra_admin', 'MEMBER') THEN
        RAISE EXCEPTION 'only an administrator can manage users' USING ERRCODE = 'H0001';
    END IF;
    IF username IS NULL THEN
        RETURN;
    END IF;
    IF NOT coalesce(username ~ '^[a-z][a-z0-9_]{0,62}$', false) THEN
        RAISE EXCEPTION '"%" is not a user name: a lowercase letter, then up to 62 lowercase letters, digits or _',
            username USING ERRCODE = 'H0001';
    END IF;
    IF pg_catalog.left(username, 3) = 'pg_' THEN
        RAISE EXCEPTION 'user names can''t start with pg_' USING ERRCODE = 'H0001';
    END IF;
    IF username IN ('public', 'none', 'current_user', 'current_role', 'session_user', 'user') THEN
        RAISE EXCEPTION '"%" means something else to Postgres; choose another user name',
            username USING ERRCODE = 'H0001';
    END IF;
    IF username IN ('hydra_engineer', 'hydra_admin', 'hydra_manager') THEN
        RAISE EXCEPTION '% is one of Hydra''s own roles, not a user', username USING ERRCODE = 'H0001';
    END IF;
    IF action IS NULL THEN
        RETURN;
    END IF;
    -- Users are members of hydra_engineer, directly or through hydra_admin, and nothing more:
    -- never a superuser, a role with rights of its own or other memberships.
    SELECT * INTO found FROM pg_catalog.pg_roles r WHERE r.rolname = username;
    IF NOT EXISTS (SELECT FROM pg_catalog.pg_auth_members a
                   WHERE a.member = found.oid
                     AND a.roleid IN ('hydra_engineer'::pg_catalog.regrole,
                                      'hydra_admin'::pg_catalog.regrole)) THEN
        RAISE EXCEPTION 'unknown user %', username USING ERRCODE = 'H0001';
    END IF;
    IF found.rolsuper OR found.rolcreaterole OR found.rolcreatedb OR found.rolreplication
        OR found.rolbypassrls
        OR EXISTS (SELECT FROM pg_catalog.pg_auth_members a
                   WHERE a.member = found.oid
                     AND a.roleid NOT IN ('hydra_engineer'::pg_catalog.regrole,
                                          'hydra_admin'::pg_catalog.regrole)) THEN
        RAISE EXCEPTION '% has database rights beyond a Hydra user''s; only a database superuser can change this user',
            username USING ERRCODE = 'H0001';
    END IF;
    -- Nor objects they own, or privileges or policies of their own. Any user can own a
    -- temporary table, so this never stops taking rights away.
    IF action <> 'take' AND EXISTS (SELECT FROM pg_catalog.pg_shdepend d
                                    WHERE d.refclassid = 'pg_catalog.pg_authid'::pg_catalog.regclass
                                      AND d.refobjid = found.oid AND d.deptype IN ('o', 'a', 'r')) THEN
        IF action = 'delete' THEN
            RAISE EXCEPTION '% owns database objects or holds privileges of their own; a database superuser must reassign or drop them first (temporary ones go when their sessions end)',
                username USING ERRCODE = 'H0001';
        END IF;
        RAISE EXCEPTION '% has database rights beyond a Hydra user''s; only a database superuser can change this user',
            username USING ERRCODE = 'H0001';
    END IF;
    -- PostgreSQL 16+ lets hydra_manager change only roles it holds ADMIN OPTION on: it gets
    -- that on the users it creates, and ops/roles.sql gives it on users added by hand.
    IF pg_catalog.current_setting('server_version_num')::integer >= 160000
       AND NOT pg_catalog.pg_has_role(found.oid, 'USAGE WITH ADMIN OPTION') THEN
        RAISE EXCEPTION '% was added outside Hydra; ops/roles.sql must run again before administrators can change this user',
            username USING ERRCODE = 'H0001';
    END IF;
    IF username = session_user AND refusal IS NOT NULL THEN
        RAISE EXCEPTION '%', refusal USING ERRCODE = 'H0001';
    END IF;
END
$$;

-- Refuses if `username` is the last administrator who can sign in. One such change at a time,
-- so two administrators can't remove each other at once.
CREATE FUNCTION hydra_auth.hydra_keep_an_admin(username text)
RETURNS void LANGUAGE plpgsql SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    target boolean;
    others boolean;
BEGIN
    PERFORM pg_catalog.pg_advisory_xact_lock(448682234465); -- "hydra" in ASCII
    WITH RECURSIVE members(oid) AS (
        SELECT a.member FROM pg_catalog.pg_auth_members a
        WHERE a.roleid = 'hydra_admin'::pg_catalog.regrole
        UNION SELECT a.member FROM pg_catalog.pg_auth_members a JOIN members m ON a.roleid = m.oid)
    SELECT pg_catalog.bool_or(r.rolname = username), pg_catalog.bool_or(r.rolname <> username)
    INTO target, others
    FROM pg_catalog.pg_roles r
    WHERE r.oid IN (SELECT m.oid FROM members m) AND r.rolcanlogin;
    IF target AND NOT others THEN
        RAISE EXCEPTION 'Hydra needs at least one administrator' USING ERRCODE = 'H0001';
    END IF;
END
$$;

-- `verifier` if it is a SCRAM-SHA-256 verifier Postgres can read, so a password never arrives
-- as plain text: Postgres would store one it can't read as a plain-text password.
CREATE FUNCTION hydra_auth.hydra_verifier(verifier text)
RETURNS text LANGUAGE plpgsql IMMUTABLE SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    parts text[] := pg_catalog.regexp_match(verifier,
        '^SCRAM-SHA-256\$([0-9]{1,9}):([A-Za-z0-9+/=]+)\$([A-Za-z0-9+/=]+):([A-Za-z0-9+/=]+)$');
    readable boolean;
BEGIN
    BEGIN
        readable := parts[1]::integer BETWEEN 4096 AND 100000
            AND pg_catalog.length(pg_catalog.decode(parts[2], 'base64')) >= 8
            AND pg_catalog.length(pg_catalog.decode(parts[3], 'base64')) = 32
            AND pg_catalog.length(pg_catalog.decode(parts[4], 'base64')) = 32;
    EXCEPTION WHEN invalid_parameter_value THEN -- not base64
        readable := false;
    END;
    IF NOT coalesce(readable, false) THEN
        RAISE EXCEPTION 'a password must arrive as a SCRAM-SHA-256 verifier' USING ERRCODE = 'H0001';
    END IF;
    RETURN verifier;
END
$$;

-- Anyone's own password. Runs as the caller, who may always change their own.
CREATE FUNCTION hydra_auth.hydra_set_own_password(verifier text)
RETURNS void LANGUAGE plpgsql STRICT SET search_path = pg_catalog, pg_temp AS $$
BEGIN
    EXECUTE pg_catalog.format('ALTER ROLE %I PASSWORD %L', session_user,
                              hydra_auth.hydra_verifier(verifier));
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION USING ERRCODE = SQLSTATE, MESSAGE = SQLERRM;
END
$$;

-- One row per user and open session (session columns null without one), by name, then
-- oldest session first.
CREATE FUNCTION hydra_auth.hydra_list_users()
RETURNS TABLE (username text, admin boolean, can_login boolean, pid integer, application text,
               client text, since text)
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
BEGIN
    PERFORM hydra_auth.hydra_user_check(NULL, NULL, NULL);
    -- A superuser counts as a member of every role in pg_has_role: walk the grants instead.
    RETURN QUERY
        WITH RECURSIVE members(oid) AS (
            SELECT a.member FROM pg_catalog.pg_auth_members a
            WHERE a.roleid = 'hydra_engineer'::pg_catalog.regrole
            UNION SELECT a.member FROM pg_catalog.pg_auth_members a JOIN members m ON a.roleid = m.oid)
        SELECT r.rolname::text, pg_catalog.pg_has_role(r.oid, 'hydra_admin', 'MEMBER'), r.rolcanlogin,
               s.pid, coalesce(s.application_name, ''), pg_catalog.host(s.client_addr),
               pg_catalog.to_char(s.backend_start, 'YYYY-MM-DD HH24:MI')
        FROM pg_catalog.pg_roles r
        LEFT JOIN pg_catalog.pg_stat_activity s
            ON s.usesysid = r.oid AND s.backend_type = 'client backend'
        WHERE r.oid IN (SELECT m.oid FROM members m)
          AND r.rolname NOT IN ('hydra_admin', 'hydra_manager')
        ORDER BY r.rolname, s.backend_start, s.pid;
END
$$;

CREATE FUNCTION hydra_auth.hydra_create_user(username text, verifier text, admin boolean)
RETURNS void LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
BEGIN
    PERFORM hydra_auth.hydra_user_check(username, NULL, NULL);
    IF EXISTS (SELECT FROM pg_catalog.pg_roles r WHERE r.rolname = username) THEN
        RAISE EXCEPTION 'a user or role named % already exists', username USING ERRCODE = 'H0001';
    END IF;
    -- PostgreSQL 16+ would also grant the new role to hydra_manager as the caller's
    -- createrole_self_grant says; it needs only the ADMIN OPTION it gets anyway.
    IF pg_catalog.current_setting('server_version_num')::integer >= 160000 THEN
        PERFORM pg_catalog.set_config('createrole_self_grant', '', true);
    END IF;
    BEGIN
        EXECUTE pg_catalog.format('CREATE ROLE %I LOGIN PASSWORD %L IN ROLE hydra_engineer',
                                  username, hydra_auth.hydra_verifier(verifier));
    EXCEPTION WHEN OTHERS THEN
        RAISE EXCEPTION USING ERRCODE = SQLSTATE, MESSAGE = SQLERRM;
    END;
    IF admin THEN
        EXECUTE pg_catalog.format('GRANT hydra_admin TO %I', username);
    END IF;
END
$$;

CREATE FUNCTION hydra_auth.hydra_set_password(username text, verifier text)
RETURNS void LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
BEGIN
    PERFORM hydra_auth.hydra_user_check(username, 'give', NULL);
    BEGIN
        EXECUTE pg_catalog.format('ALTER ROLE %I PASSWORD %L', username,
                                  hydra_auth.hydra_verifier(verifier));
    EXCEPTION WHEN OTHERS THEN
        RAISE EXCEPTION USING ERRCODE = SQLSTATE, MESSAGE = SQLERRM;
    END;
END
$$;

CREATE FUNCTION hydra_auth.hydra_set_admin(username text, admin boolean)
RETURNS void LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    -- Whether username is a direct member of the group $2.
    member text := $q$SELECT EXISTS (SELECT FROM pg_catalog.pg_auth_members a
                                     WHERE a.roleid = $2::pg_catalog.regrole
                                       AND a.member = $1::pg_catalog.regrole)$q$;
    is_admin boolean;
    is_engineer boolean;
BEGIN
    IF admin THEN
        PERFORM hydra_auth.hydra_user_check(username, 'give', NULL);
    ELSE
        PERFORM hydra_auth.hydra_user_check(username, 'take',
                                            'you can''t remove your own administrator rights');
        PERFORM hydra_auth.hydra_keep_an_admin(username);
    END IF;
    EXECUTE member INTO is_admin USING username, 'hydra_admin';
    IF admin AND NOT is_admin THEN
        EXECUTE pg_catalog.format('GRANT hydra_admin TO %I', username);
    ELSIF is_admin AND NOT admin THEN
        -- An admin added by hand may be an engineer only through hydra_admin.
        EXECUTE member INTO is_engineer USING username, 'hydra_engineer';
        IF NOT is_engineer THEN
            EXECUTE pg_catalog.format('GRANT hydra_engineer TO %I', username);
        END IF;
        EXECUTE pg_catalog.format('REVOKE hydra_admin FROM %I', username);
        -- PostgreSQL 16+ keeps one grant per grantor; this only takes back hydra_manager's.
        EXECUTE member INTO is_admin USING username, 'hydra_admin';
        IF is_admin THEN
            RAISE EXCEPTION '% was made an administrator outside Hydra; ops/roles.sql must run again first',
                username USING ERRCODE = 'H0001';
        END IF;
    END IF;
END
$$;

-- Ends every session of `username`, waiting up to 5 s for each; returns how many ended.
CREATE FUNCTION hydra_auth.hydra_end_sessions(username text)
RETURNS integer LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    ended integer;
BEGIN
    PERFORM hydra_auth.hydra_user_check(username, 'take', 'you can''t end your own sessions');
    SELECT count(*) FILTER (WHERE pg_catalog.pg_terminate_backend(s.pid, 5000)) INTO ended
    FROM pg_catalog.pg_stat_activity s
    WHERE s.backend_type = 'client backend' AND s.usename = username;
    RETURN ended;
END
$$;

-- Disabling ends no sessions: end them once this has committed, so none signs in again first.
CREATE FUNCTION hydra_auth.hydra_set_can_login(username text, can_login boolean)
RETURNS void LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
BEGIN
    IF can_login THEN
        PERFORM hydra_auth.hydra_user_check(username, 'give', NULL);
        EXECUTE pg_catalog.format('ALTER ROLE %I LOGIN', username);
    ELSE
        PERFORM hydra_auth.hydra_user_check(username, 'take', 'you can''t disable yourself');
        PERFORM hydra_auth.hydra_keep_an_admin(username);
        EXECUTE pg_catalog.format('ALTER ROLE %I NOLOGIN', username);
    END IF;
END
$$;

-- Drops the role and returns its oid; once that has committed, hydra_end_deleted_sessions
-- ends the sessions it had (so a refused drop signs nobody out). Their name stays on what
-- they saved.
CREATE FUNCTION hydra_auth.hydra_delete_user(username text)
RETURNS oid LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    deleted oid;
BEGIN
    PERFORM hydra_auth.hydra_user_check(username, 'delete', 'you can''t delete yourself');
    PERFORM hydra_auth.hydra_keep_an_admin(username);
    SELECT r.oid INTO deleted FROM pg_catalog.pg_roles r WHERE r.rolname = username;
    EXECUTE pg_catalog.format('DROP ROLE %I', username);
    RETURN deleted;
END
$$;

-- Ends the sessions still open for the role with oid `deleted`, only once no role has it.
CREATE FUNCTION hydra_auth.hydra_end_deleted_sessions(deleted oid)
RETURNS integer LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    ended integer;
BEGIN
    PERFORM hydra_auth.hydra_user_check(NULL, NULL, NULL);
    IF EXISTS (SELECT FROM pg_catalog.pg_roles r WHERE r.oid = deleted) THEN
        RAISE EXCEPTION 'that user still exists' USING ERRCODE = 'H0001';
    END IF;
    SELECT count(*) FILTER (WHERE pg_catalog.pg_terminate_backend(s.pid, 5000)) INTO ended
    FROM pg_catalog.pg_stat_activity s
    WHERE s.backend_type = 'client backend' AND s.usesysid = deleted;
    RETURN ended;
END
$$;

-- hydra_verifier and hydra_set_own_password stay open to everyone: they act only on the caller.
REVOKE ALL ON FUNCTION
    hydra_auth.hydra_user_check(text, text, text), hydra_auth.hydra_keep_an_admin(text),
    hydra_auth.hydra_list_users(), hydra_auth.hydra_create_user(text, text, boolean),
    hydra_auth.hydra_set_password(text, text), hydra_auth.hydra_set_admin(text, boolean),
    hydra_auth.hydra_end_sessions(text), hydra_auth.hydra_set_can_login(text, boolean),
    hydra_auth.hydra_delete_user(text), hydra_auth.hydra_end_deleted_sessions(oid)
FROM PUBLIC;
