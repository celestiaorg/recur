package store

import (
	"database/sql"
	"embed"
	"fmt"
	"io/fs"
	"sort"
	"strconv"
	"strings"
	"time"
)

// migrationFiles holds the ordered, versioned schema migrations as plain .sql
// files under migrations/. Each file is named NNNN_name.sql (zero-padded version,
// underscore, human name). They are embedded into the binary at build time, so
// the shipped app carries its own schema and no files need to travel alongside it.
//
// NEVER edit an already-shipped migration in place (a deployed database has
// recorded its version and will not re-run it) — add a new NNNN_*.sql file with
// the next number instead.
//
//go:embed migrations/*.sql
var migrationFiles embed.FS

// migration is one parsed migration file.
type migration struct {
	version int
	name    string
	stmts   string
}

// loadMigrations reads and parses the embedded migration files in version order.
// A malformed filename is a programming error (the files are ours), so it fails
// loudly rather than silently skipping a schema change.
func loadMigrations() ([]migration, error) {
	entries, err := fs.ReadDir(migrationFiles, "migrations")
	if err != nil {
		return nil, fmt.Errorf("read embedded migrations: %w", err)
	}
	var out []migration
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".sql") {
			continue
		}
		version, name, err := parseMigrationName(e.Name())
		if err != nil {
			return nil, err
		}
		body, err := fs.ReadFile(migrationFiles, "migrations/"+e.Name())
		if err != nil {
			return nil, fmt.Errorf("read migration %s: %w", e.Name(), err)
		}
		out = append(out, migration{version: version, name: name, stmts: string(body)})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].version < out[j].version })
	for i := 1; i < len(out); i++ {
		if out[i].version == out[i-1].version {
			return nil, fmt.Errorf("duplicate migration version %d (%s / %s)",
				out[i].version, out[i-1].name, out[i].name)
		}
	}
	return out, nil
}

// parseMigrationName splits "0002_sessions.sql" into (2, "sessions").
func parseMigrationName(file string) (int, string, error) {
	base := strings.TrimSuffix(file, ".sql")
	num, rest, ok := strings.Cut(base, "_")
	if !ok {
		return 0, "", fmt.Errorf("migration %q: want NNNN_name.sql", file)
	}
	version, err := strconv.Atoi(num)
	if err != nil {
		return 0, "", fmt.Errorf("migration %q: bad version prefix: %w", file, err)
	}
	return version, rest, nil
}

// runMigrations brings the database up to the latest schema version. It sets the
// connection PRAGMAs, ensures the __migrations bookkeeping table exists, then
// applies each not-yet-applied migration in version order inside its own
// transaction. Idempotent: on an up-to-date DB it does nothing but the PRAGMAs.
func (d *db) runMigrations() error {
	// Connection-level PRAGMAs (set on the single pooled connection every open, not
	// persisted in a migration): WAL for concurrent readers, foreign_keys for the
	// ON DELETE CASCADE relations the session tables declare.
	if _, err := d.sql.Exec(`PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;`); err != nil {
		return fmt.Errorf("set connection pragmas: %w", err)
	}
	if _, err := d.sql.Exec(`CREATE TABLE IF NOT EXISTS __migrations (
		version    INTEGER PRIMARY KEY,
		name       TEXT NOT NULL,
		applied_at INTEGER NOT NULL
	)`); err != nil {
		return fmt.Errorf("create __migrations: %w", err)
	}

	migrations, err := loadMigrations()
	if err != nil {
		return err
	}
	applied, err := d.appliedMigrations()
	if err != nil {
		return err
	}
	for _, m := range migrations {
		if applied[m.version] {
			continue
		}
		if err := d.applyMigration(m); err != nil {
			return err
		}
	}
	return nil
}

// appliedMigrations returns the set of migration versions already recorded.
func (d *db) appliedMigrations() (map[int]bool, error) {
	rows, err := d.sql.Query(`SELECT version FROM __migrations`)
	if err != nil {
		return nil, fmt.Errorf("query __migrations: %w", err)
	}
	defer rows.Close()
	out := map[int]bool{}
	for rows.Next() {
		var v int
		if err := rows.Scan(&v); err != nil {
			return nil, fmt.Errorf("scan __migrations: %w", err)
		}
		out[v] = true
	}
	return out, rows.Err()
}

// applyMigration runs one migration's statements and records its version in a
// single transaction, so a partially-applied migration can never be recorded.
func (d *db) applyMigration(m migration) error {
	tx, err := d.sql.Begin()
	if err != nil {
		return fmt.Errorf("begin migration %d (%s): %w", m.version, m.name, err)
	}
	defer tx.Rollback() //nolint:errcheck // no-op after Commit

	if _, err := tx.Exec(m.stmts); err != nil {
		return fmt.Errorf("apply migration %d (%s): %w", m.version, m.name, err)
	}
	if _, err := tx.Exec(
		`INSERT INTO __migrations (version, name, applied_at) VALUES (?,?,?)`,
		m.version, m.name, time.Now().UnixMilli(),
	); err != nil {
		return fmt.Errorf("record migration %d (%s): %w", m.version, m.name, err)
	}
	return tx.Commit()
}

// schemaVersion returns the highest applied migration version (0 on a DB with no
// migrations recorded). Exposed for tests.
func (d *db) schemaVersion() (int, error) {
	var v sql.NullInt64
	if err := d.sql.QueryRow(`SELECT MAX(version) FROM __migrations`).Scan(&v); err != nil {
		return 0, fmt.Errorf("read schema version: %w", err)
	}
	if !v.Valid {
		return 0, nil
	}
	return int(v.Int64), nil
}

// latestSchemaVersion returns the highest version among the embedded migrations —
// what schemaVersion should equal after a successful open. Exposed for tests.
func latestSchemaVersion() int {
	ms, err := loadMigrations()
	if err != nil || len(ms) == 0 {
		return 0
	}
	return ms[len(ms)-1].version
}
