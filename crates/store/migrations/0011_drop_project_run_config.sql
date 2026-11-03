-- Run-config (setup, teardown, named runs, app URL, env) lives only in
-- <repo>/.recur/project.toml. Drop the vestigial project columns so a
-- missing file cannot resurrect stale DB values.
ALTER TABLE projects DROP COLUMN setup_cmd;
ALTER TABLE projects DROP COLUMN teardown_cmd;
ALTER TABLE projects DROP COLUMN dev_cmd;
ALTER TABLE projects DROP COLUMN app_url;
