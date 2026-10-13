-- ReOpenSCAD workspace schema.
--
-- This file is documentation, not the deployment mechanism: the server applies
-- exactly these statements itself at startup (see `MIGRATIONS` in
-- `backend/src/pgstore.rs`), idempotently, under a Postgres advisory lock so
-- that a fleet of instances cold-starting at once cannot race each other. It
-- is reproduced here so the schema can be reviewed, diffed, or applied by hand
-- against a restored backup without reading Rust.
--
-- Applying it manually is safe and makes the startup migration a no-op.

-- Which migrations have been applied. Versions are appended, never edited.
CREATE TABLE IF NOT EXISTS reopenscad_schema (
    version    INTEGER PRIMARY KEY,
    applied_at BIGINT NOT NULL          -- unix milliseconds
);

-- One row per workspace. A workspace is a set of SCAD files, one of which
-- renders, plus the viewer settings and the recent edit log; the URL containing
-- `id` is the only access control there is, so `id` carries 112 bits of entropy.
CREATE TABLE IF NOT EXISTS workspaces (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,

    -- `{"name.scad": "source", ...}`, every file including the main one. The
    -- sources together are bounded by MAX_BODY (2 MiB) in the server. JSONB in
    -- the row rather than a child table so an edit stays the single-statement
    -- compare-and-swap described under `revision`.
    files      JSONB NOT NULL,
    main       TEXT NOT NULL DEFAULT 'main.scad',   -- the key of `files` that renders

    -- A copy of the main file, kept for builds from before `files` existed
    -- (rolling deploys and rollbacks). The server writes it and never reads
    -- it; `workspaces_sync_legacy_code` below carries an older build's writes
    -- across into `files`.
    code       TEXT NOT NULL,

    -- The optimistic-concurrency token. Every edit is
    --   UPDATE ... SET revision = revision + 1 WHERE id = $1 AND revision = $2
    -- so the compare-and-swap happens under Postgres's row lock rather than in
    -- one process's mutex. That is what makes multi-instance serving safe: a
    -- second writer holding the same base revision matches zero rows and is
    -- answered with 409 instead of silently overwriting the first.
    revision   BIGINT NOT NULL,

    created_at BIGINT NOT NULL,         -- unix milliseconds
    updated_at BIGINT NOT NULL,         -- unix milliseconds; drives TTL pruning

    -- Viewer settings. Revisionless: they are written unconditionally, so
    -- picking a build plate can never conflict with an in-flight code save.
    plate      TEXT NOT NULL DEFAULT '',
    tolerance  DOUBLE PRECISION NOT NULL DEFAULT 0.2,

    -- A small render thumbnail as a base64 `data:` URL, written by the browser
    -- that drew it and shown in the "recent workspaces" list. Empty until a
    -- render has happened; bounded by MAX_PREVIEW_BYTES (96 KiB) in the server.
    preview    TEXT NOT NULL DEFAULT '',

    -- Capability for the read-only view, or '' until the workspace is shared.
    -- Independent of `id` rather than derived from it: holding the read-only
    -- link must not get you the editable one.
    share_token TEXT NOT NULL DEFAULT '',

    -- The recent edit log the `/events` long poll replays to other tabs and to
    -- the browser after an MCP agent writes. Capped at 64 entries by the
    -- UPDATE statement itself, so the bound holds across instances. Kept in the
    -- workspace row rather than a child table because every read of it is
    -- "the tail of the log for this one workspace", which is exactly what a
    -- single row already answers.
    events     JSONB NOT NULL DEFAULT '[]'::jsonb
);

-- Pruning orders by `updated_at` (TTL sweep and the count cap), and without
-- this both are sequential scans of the whole table every ten minutes.
-- Workspace reads need no index: they are primary-key lookups.
-- Unique where it exists, so two workspaces can never answer to the same
-- read-only link. Partial, so every unshared row can keep the same '' default.
CREATE UNIQUE INDEX IF NOT EXISTS workspaces_share_token_idx
    ON workspaces (share_token) WHERE share_token <> '';

CREATE INDEX IF NOT EXISTS workspaces_updated_at_idx
    ON workspaces (updated_at DESC, id DESC);

-- See the comment on `code`. An INSERT without `files`, or an UPDATE that
-- changes `code` but not `files`, can only come from an older build.
CREATE OR REPLACE FUNCTION reopenscad_sync_legacy_code() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.files IS NULL THEN
            NEW.files := jsonb_build_object(NEW.main, NEW.code);
        END IF;
    ELSIF NEW.code IS DISTINCT FROM OLD.code
          AND NEW.files IS NOT DISTINCT FROM OLD.files THEN
        NEW.files := jsonb_set(NEW.files, ARRAY[NEW.main], to_jsonb(NEW.code));
    END IF;
    RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS workspaces_sync_legacy_code ON workspaces;
CREATE TRIGGER workspaces_sync_legacy_code
    BEFORE INSERT OR UPDATE ON workspaces
    FOR EACH ROW EXECUTE FUNCTION reopenscad_sync_legacy_code();

INSERT INTO reopenscad_schema (version, applied_at)
VALUES (1, (EXTRACT(EPOCH FROM now()) * 1000)::BIGINT)
ON CONFLICT (version) DO NOTHING;

-- Retention, for reference. The server runs both statements on a ten-minute
-- sweeper thread; neither loads a row into the process.
--
--   -- 14-day idle TTL
--   DELETE FROM workspaces WHERE updated_at < $ttl_cutoff_millis;
--
--   -- 512-workspace cap, oldest first
--   DELETE FROM workspaces WHERE id IN (
--       SELECT id FROM workspaces ORDER BY updated_at DESC, id DESC OFFSET 512
--   );
