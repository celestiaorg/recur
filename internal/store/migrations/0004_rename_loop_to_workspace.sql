-- 0004_rename_loop_to_workspace — unify the model: Loop -> Workspace (one noun,
-- several types). Renames the v1 loop-named doc table + loop_id columns so an
-- EXISTING database migrates in place with no data loss; fresh databases created
-- the loop names in 0001 too, so this runs identically for both.
--
-- Numbered 4 (skipping 3) on purpose: a transient parallel-Workspace build shipped
-- a different migration 3 that some dev databases recorded and that created a stray
-- `workspaces` doc-table. Drop that first so the rename below can proceed; it held
-- only throwaway solo-workspace test rows. DROP IF EXISTS is a no-op on a fresh DB
-- (whose `workspaces` doesn't exist yet — its data is still in `loops`).

DROP TABLE IF EXISTS workspaces;

ALTER TABLE loops RENAME TO workspaces;

ALTER TABLE agents       RENAME COLUMN loop_id TO workspace_id;
ALTER TABLE feed         RENAME COLUMN loop_id TO workspace_id;
ALTER TABLE escalations  RENAME COLUMN loop_id TO workspace_id;
ALTER TABLE maestro_chat RENAME COLUMN loop_id TO workspace_id;
ALTER TABLE events       RENAME COLUMN loop_id TO workspace_id;
ALTER TABLE findings     RENAME COLUMN loop_id TO workspace_id;
ALTER TABLE analysis     RENAME COLUMN loop_id TO workspace_id;

-- The session table already carries workspace_id (added in 0002); its unused
-- loop_id column is left in place (harmless) rather than dropped here, since
-- SQLite column drops are heavier and nothing reads it.
