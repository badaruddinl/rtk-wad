use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::state;

const LEDGER_SCHEMA_VERSION: i64 = 1;
const MAX_LEDGER_INVOCATIONS: i64 = 10_000;
const TRACKER_TEMPLATE_NAME: &str = "tracker-template-v2.sqlite";

fn open_database(path: &Path, purpose: &str) -> Result<Connection, String> {
    let existed = path.exists();
    let connection =
        Connection::open(path).map_err(|error| format!("unable to open {purpose}: {error}"))?;
    if let Err(error) = state::secure_private_path(path, purpose) {
        drop(connection);
        if !existed {
            remove_scratch_database(path);
        }
        return Err(error);
    }
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("unable to configure {purpose} concurrency: {error}"))?;
    Ok(connection)
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TokenTotals {
    pub(crate) commands: i64,
    pub(crate) input_tokens: i64,
    pub(crate) output_tokens: i64,
    pub(crate) saved_tokens: i64,
}

pub(crate) fn xuva_data_root() -> PathBuf {
    env::var_os("XUVA_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|root| root.join("xuva"))
        })
        .unwrap_or_else(|| env::temp_dir().join("xuva"))
}

pub(crate) struct XuvaMetrics {
    ledger_path: PathBuf,
    scratch_path: PathBuf,
}

impl XuvaMetrics {
    pub(crate) fn begin() -> Result<Self, String> {
        Self::begin_with_tracker(true)
    }

    pub(crate) fn begin_unmeasured() -> Result<Self, String> {
        Self::begin_with_tracker(false)
    }

    fn begin_with_tracker(with_tracker: bool) -> Result<Self, String> {
        let root = xuva_data_root();
        fs::create_dir_all(&root)
            .map_err(|error| format!("unable to create local XUVA state directory: {error}"))?;
        state::secure_private_path(&root, "local XUVA state directory")?;
        let scratch_directory = root.join("scratch");
        fs::create_dir_all(&scratch_directory)
            .map_err(|error| format!("unable to create local metrics directory: {error}"))?;
        state::secure_private_path(&scratch_directory, "local metrics directory")?;
        cleanup_stale_scratch(&scratch_directory);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let scratch_path = scratch_directory.join(format!("{}-{nonce}.sqlite", std::process::id()));
        let ledger_path = root.join("metrics-v1.sqlite");
        let metrics = Self {
            ledger_path,
            scratch_path,
        };
        if with_tracker {
            let tracker_template = root.join(TRACKER_TEMPLATE_NAME);
            if !tracker_template.exists() {
                initialize_tracker_template(&tracker_template)?;
            }
            fs::copy(&tracker_template, &metrics.scratch_path)
                .map_err(|error| format!("unable to prepare temporary RTK metrics: {error}"))?;
            state::secure_private_path(&metrics.scratch_path, "temporary RTK metrics")?;
        }
        metrics.initialize_ledger()?;
        Ok(metrics)
    }

    fn initialize_ledger(&self) -> Result<(), String> {
        let connection = open_database(&self.ledger_path, "local metrics ledger")?;
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| format!("unable to inspect local metrics schema: {error}"))?;
        if schema_version > LEDGER_SCHEMA_VERSION {
            return Err(format!(
                "local metrics schema {schema_version} is newer than supported version {LEDGER_SCHEMA_VERSION}"
            ));
        }
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE IF NOT EXISTS invocations (
                    id INTEGER PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    route TEXT NOT NULL,
                    command_family TEXT NOT NULL,
                    commands INTEGER NOT NULL,
                    input_tokens INTEGER NOT NULL,
                    output_tokens INTEGER NOT NULL,
                    saved_tokens INTEGER NOT NULL,
                    elapsed_ms INTEGER NOT NULL,
                    exit_code INTEGER NOT NULL,
                    measured INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_invocations_timestamp ON invocations(timestamp);
                 PRAGMA user_version=1;",
            )
            .map_err(|error| format!("unable to initialize local metrics ledger: {error}"))
    }

    pub(crate) fn scratch_windows_path(&self) -> &Path {
        &self.scratch_path
    }

    pub(crate) fn finish(
        self,
        route: &str,
        command_family: &str,
        elapsed: Duration,
        exit_code: i32,
    ) -> Result<TokenTotals, String> {
        let totals = read_upstream_totals(&self.scratch_path)?;
        let measured = i64::from(totals.commands > 0);
        let mut connection = open_database(&self.ledger_path, "local metrics ledger")?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("unable to begin local metrics transaction: {error}"))?;
        transaction
            .execute(
                "INSERT INTO invocations (timestamp, route, command_family, commands, input_tokens, output_tokens, saved_tokens, elapsed_ms, exit_code, measured)
                 VALUES (datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    route,
                    command_family,
                    totals.commands,
                    totals.input_tokens,
                    totals.output_tokens,
                    totals.saved_tokens,
                    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
                    exit_code,
                    measured,
                ],
            )
            .map_err(|error| format!("unable to record local metrics: {error}"))?;
        transaction
            .execute(
                "DELETE FROM invocations WHERE id <= COALESCE((SELECT MAX(id) FROM invocations), 0) - ?1",
                [MAX_LEDGER_INVOCATIONS],
            )
            .map_err(|error| format!("unable to enforce local metrics retention: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("unable to commit local metrics: {error}"))?;
        Ok(totals)
    }

    pub(crate) fn print_gain() -> Result<(), String> {
        let root = xuva_data_root();
        let ledger_path = root.join("metrics-v1.sqlite");
        if !ledger_path.exists() {
            println!("XUVA Measured Token Accounting\n\nNo RTK-measured commands yet.");
            return Ok(());
        }
        let connection = open_database(&ledger_path, "local metrics ledger")?;
        let totals = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(commands), 0), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(saved_tokens), 0), COALESCE(SUM(measured), 0) FROM invocations",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?)),
            )
            .map_err(|error| format!("unable to read local metrics ledger: {error}"))?;
        let savings = if totals.2 > 0 {
            (totals.4 as f64 / totals.2 as f64) * 100.0
        } else {
            0.0
        };
        let unmeasured = totals.0.saturating_sub(totals.5);
        println!("XUVA Measured Token Accounting");
        println!();
        println!(
            "Invocations: {} ({} RTK-measured, {} unmeasured)",
            totals.0, totals.5, unmeasured
        );
        println!("RTK-measured commands: {}", totals.1);
        println!("RTK input tokens: {}", totals.2);
        println!("RTK output tokens: {}", totals.3);
        println!("RTK-reported tokens avoided: {} ({savings:.1}%)", totals.4);
        println!();
        println!("By route (RTK-reported accounting only):");
        let mut statement = connection
            .prepare("SELECT route, COUNT(*), COALESCE(SUM(commands), 0), COALESCE(SUM(saved_tokens), 0), COALESCE(SUM(measured), 0) FROM invocations GROUP BY route ORDER BY saved_tokens DESC, route")
            .map_err(|error| format!("unable to prepare local metrics summary: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|error| format!("unable to read local metrics summary: {error}"))?;
        for row in rows {
            let (route, count, commands, saved, measured) =
                row.map_err(|error| format!("unable to decode local metrics summary: {error}"))?;
            println!(
                "  {route}: {count} invocation(s), {measured} RTK-measured, {commands} measured command(s), {saved} RTK-reported tokens avoided"
            );
        }
        Ok(())
    }

    pub(crate) fn purge() -> Result<usize, String> {
        purge_from(&xuva_data_root())
    }
}

fn purge_from(root: &Path) -> Result<usize, String> {
    let mut removed = 0;
    for name in [
        "metrics-v1.sqlite",
        TRACKER_TEMPLATE_NAME,
        "tracker-template.sqlite",
    ] {
        removed += remove_database_family(&root.join(name), "local metrics file")?;
    }
    let scratch = root.join("scratch");
    if scratch.exists() {
        let metadata = fs::symlink_metadata(&scratch)
            .map_err(|error| format!("unable to inspect local metrics scratch: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "refusing to purge unsafe local metrics scratch path: {}",
                scratch.display()
            ));
        }
        for entry in fs::read_dir(&scratch)
            .map_err(|error| format!("unable to inspect local metrics scratch: {error}"))?
        {
            let entry = entry
                .map_err(|error| format!("unable to inspect local metrics scratch: {error}"))?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                format!("unable to inspect local metrics scratch entry: {error}")
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "refusing to purge unsafe local metrics scratch entry: {}",
                    entry.path().display()
                ));
            }
            fs::remove_file(entry.path())
                .map_err(|error| format!("unable to purge local metrics scratch: {error}"))?;
            removed += 1;
        }
        if fs::read_dir(&scratch)
            .map_err(|error| format!("unable to verify local metrics scratch: {error}"))?
            .next()
            .is_none()
        {
            fs::remove_dir(&scratch).map_err(|error| {
                format!("unable to remove local metrics scratch directory: {error}")
            })?;
        }
    }
    Ok(removed)
}

impl Drop for XuvaMetrics {
    fn drop(&mut self) {
        remove_scratch_database(&self.scratch_path);
    }
}

fn initialize_tracker_template(path: &Path) -> Result<(), String> {
    let connection = open_database(path, "RTK metrics template")?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS command_totals (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                saved_tokens INTEGER NOT NULL,
                savings_pct REAL NOT NULL,
                exec_time_ms INTEGER DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_command_totals_timestamp ON command_totals(timestamp);
             CREATE VIEW IF NOT EXISTS commands AS
                SELECT id, timestamp, '[redacted]' AS original_cmd, '[redacted]' AS rtk_cmd,
                       input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms,
                       '' AS project_path
                FROM command_totals;
             CREATE TRIGGER IF NOT EXISTS commands_insert
                INSTEAD OF INSERT ON commands
                BEGIN
                    INSERT INTO command_totals
                        (timestamp, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms)
                    VALUES
                        (NEW.timestamp, NEW.input_tokens, NEW.output_tokens, NEW.saved_tokens,
                         NEW.savings_pct, COALESCE(NEW.exec_time_ms, 0));
                END;
             CREATE TRIGGER IF NOT EXISTS commands_update
                INSTEAD OF UPDATE ON commands
                BEGIN
                    UPDATE command_totals
                    SET timestamp = NEW.timestamp,
                        input_tokens = NEW.input_tokens,
                        output_tokens = NEW.output_tokens,
                        saved_tokens = NEW.saved_tokens,
                        savings_pct = NEW.savings_pct,
                        exec_time_ms = COALESCE(NEW.exec_time_ms, 0)
                    WHERE id = OLD.id;
                END;
             CREATE TABLE IF NOT EXISTS parse_failure_totals (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                fallback_succeeded INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_parse_failure_totals_timestamp ON parse_failure_totals(timestamp);
             CREATE VIEW IF NOT EXISTS parse_failures AS
                SELECT id, timestamp, '[redacted]' AS raw_command, '[redacted]' AS error_message,
                       fallback_succeeded
                FROM parse_failure_totals;
             CREATE TRIGGER IF NOT EXISTS parse_failures_insert
                INSTEAD OF INSERT ON parse_failures
                BEGIN
                    INSERT INTO parse_failure_totals (timestamp, fallback_succeeded)
                    VALUES (NEW.timestamp, COALESCE(NEW.fallback_succeeded, 0));
                END;
             CREATE TRIGGER IF NOT EXISTS parse_failures_update
                INSTEAD OF UPDATE ON parse_failures
                BEGIN
                    UPDATE parse_failure_totals
                    SET timestamp = NEW.timestamp,
                        fallback_succeeded = COALESCE(NEW.fallback_succeeded, 0)
                    WHERE id = OLD.id;
                END;",
        )
        .map_err(|error| format!("unable to initialize RTK metrics template: {error}"))?;
    Ok(())
}

fn read_upstream_totals(path: &Path) -> Result<TokenTotals, String> {
    if !path.exists() {
        return Ok(TokenTotals::default());
    }
    let connection = open_database(path, "temporary RTK metrics")?;
    let exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'commands' AND type IN ('table', 'view')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("unable to inspect temporary RTK metrics: {error}"))?;
    if exists == 0 {
        return Ok(TokenTotals::default());
    }
    connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(saved_tokens), 0) FROM commands",
            [],
            |row| {
                Ok(TokenTotals {
                    commands: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    saved_tokens: row.get(3)?,
                })
            },
        )
        .map_err(|error| format!("unable to aggregate temporary RTK metrics: {error}"))
}

fn remove_scratch_database(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let candidate = path_with_suffix(path, suffix);
        let _ = fs::remove_file(candidate);
    }
}

fn remove_database_family(path: &Path, label: &str) -> Result<usize, String> {
    let mut removed = 0;
    for suffix in ["", "-wal", "-shm"] {
        let candidate = path_with_suffix(path, suffix);
        let Ok(metadata) = fs::symlink_metadata(&candidate) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "refusing to purge unsafe {label}: {}",
                candidate.display()
            ));
        }
        fs::remove_file(&candidate)
            .map_err(|error| format!("unable to purge {label} {}: {error}", candidate.display()))?;
        removed += 1;
    }
    Ok(removed)
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn cleanup_stale_scratch(directory: &Path) {
    let cutoff = SystemTime::now().checked_sub(Duration::from_secs(24 * 60 * 60));
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let remove = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .zip(cutoff)
            .is_some_and(|(modified, cutoff)| modified < cutoff);
        if remove {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_finish_uses_busy_timeout_and_preserves_every_record() {
        let root = env::temp_dir().join(format!(
            "xuva-metrics-concurrency-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("metrics fixture directory is created");
        let ledger_path = root.join("metrics-v1.sqlite");
        XuvaMetrics {
            ledger_path: ledger_path.clone(),
            scratch_path: root.join("initial-scratch.sqlite"),
        }
        .initialize_ledger()
        .expect("ledger initializes");

        let workers = (0..16)
            .map(|index| {
                let ledger_path = ledger_path.clone();
                let scratch_path = root.join(format!("scratch-{index}.sqlite"));
                std::thread::spawn(move || {
                    XuvaMetrics {
                        ledger_path,
                        scratch_path,
                    }
                    .finish("raw", "fixture", Duration::from_millis(1), 0)
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker
                .join()
                .expect("metrics worker does not panic")
                .expect("metrics record is preserved");
        }
        let connection = open_database(&ledger_path, "test ledger").expect("ledger opens");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .expect("metrics count reads");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("schema version reads");
        assert_eq!(count, 16);
        assert_eq!(version, LEDGER_SCHEMA_VERSION);
        drop(connection);
        fs::remove_dir_all(root).expect("metrics fixture is removed");
    }

    #[test]
    fn tracker_contract_never_persists_command_or_error_text() {
        let root = env::temp_dir().join(format!(
            "xuva-metrics-privacy-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("privacy fixture directory");
        let tracker = root.join(TRACKER_TEMPLATE_NAME);
        initialize_tracker_template(&tracker).expect("privacy-preserving tracker initializes");
        let connection = open_database(&tracker, "test tracker").expect("tracker opens");
        let command_secret = "--header=Authorization:secret-command-value";
        let error_secret = "database-url=secret-error-value";
        connection
            .execute(
                "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms, project_path)
                 VALUES (datetime('now'), ?1, ?1, 100, 10, 90, 90.0, 1, ?1)",
                [command_secret],
            )
            .expect("upstream command insert contract remains compatible");
        connection
            .execute(
                "INSERT INTO parse_failures (timestamp, raw_command, error_message, fallback_succeeded)
                 VALUES (datetime('now'), ?1, ?2, 0)",
                [command_secret, error_secret],
            )
            .expect("upstream failure insert contract remains compatible");
        let rendered: String = connection
            .query_row("SELECT original_cmd FROM commands", [], |row| row.get(0))
            .expect("redacted command reads");
        assert_eq!(rendered, "[redacted]");
        drop(connection);
        let bytes = fs::read(&tracker).expect("tracker bytes read");
        assert!(
            !bytes
                .windows(command_secret.len())
                .any(|window| window == command_secret.as_bytes())
        );
        assert!(
            !bytes
                .windows(error_secret.len())
                .any(|window| window == error_secret.as_bytes())
        );
        fs::remove_dir_all(root).expect("privacy fixture cleanup");
    }

    #[test]
    fn drop_cleans_scratch_database_sidecars_on_error_paths() {
        let root = env::temp_dir().join(format!(
            "xuva-metrics-drop-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("drop fixture directory");
        let scratch = root.join("scratch.sqlite");
        for suffix in ["", "-wal", "-shm"] {
            fs::write(path_with_suffix(&scratch, suffix), b"sensitive scratch")
                .expect("scratch fixture");
        }
        let result = XuvaMetrics {
            ledger_path: root.join("ledger.sqlite"),
            scratch_path: scratch.clone(),
        }
        .finish("raw", "fixture", Duration::ZERO, 1);
        assert!(result.is_err());
        for suffix in ["", "-wal", "-shm"] {
            assert!(!path_with_suffix(&scratch, suffix).exists());
        }
        fs::remove_dir_all(root).expect("drop fixture cleanup");
    }

    #[test]
    fn purge_removes_only_the_bounded_metrics_surface() {
        let root = env::temp_dir().join(format!(
            "xuva-metrics-purge-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let scratch = root.join("scratch");
        fs::create_dir_all(&scratch).expect("purge fixture directory");
        fs::write(root.join("metrics-v1.sqlite"), b"ledger").expect("ledger fixture");
        fs::write(root.join(TRACKER_TEMPLATE_NAME), b"template").expect("template fixture");
        fs::write(scratch.join("invocation.sqlite"), b"scratch").expect("scratch fixture");
        fs::write(root.join("route-policy-v2.json"), b"keep").expect("unrelated fixture");
        assert_eq!(purge_from(&root).expect("metrics purge"), 3);
        assert!(root.join("route-policy-v2.json").exists());
        assert!(!scratch.exists());
        fs::remove_dir_all(root).expect("purge fixture cleanup");
    }

    #[test]
    fn ledger_retains_only_the_newest_bounded_invocations() {
        let root = env::temp_dir().join(format!(
            "xuva-metrics-retention-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("retention fixture directory");
        let ledger = root.join("metrics-v1.sqlite");
        let metrics = XuvaMetrics {
            ledger_path: ledger.clone(),
            scratch_path: root.join("scratch.sqlite"),
        };
        metrics.initialize_ledger().expect("ledger initializes");
        let connection = open_database(&ledger, "test ledger").expect("ledger opens");
        connection
            .execute_batch(
                "WITH RECURSIVE sequence(value) AS (
                    VALUES(1) UNION ALL SELECT value + 1 FROM sequence WHERE value < 10005
                 )
                 INSERT INTO invocations
                    (timestamp, route, command_family, commands, input_tokens, output_tokens,
                     saved_tokens, elapsed_ms, exit_code, measured)
                 SELECT datetime('now'), 'raw', 'fixture', 0, 0, 0, 0, 0, 0, 0 FROM sequence;",
            )
            .expect("retention fixtures insert");
        drop(connection);
        metrics
            .finish("raw", "fixture", Duration::ZERO, 0)
            .expect("retention enforcement succeeds");
        let connection = open_database(&ledger, "test ledger").expect("ledger reopens");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .expect("retained count reads");
        assert_eq!(count, MAX_LEDGER_INVOCATIONS);
        drop(connection);
        fs::remove_dir_all(root).expect("retention fixture cleanup");
    }
}
