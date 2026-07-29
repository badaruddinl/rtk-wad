use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

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
        .unwrap_or_else(env::temp_dir)
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
        let scratch_directory = root.join("scratch");
        fs::create_dir_all(&scratch_directory)
            .map_err(|error| format!("unable to create local metrics directory: {error}"))?;
        cleanup_stale_scratch(&scratch_directory);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let scratch_path = scratch_directory.join(format!("{}-{nonce}.sqlite", std::process::id()));
        if with_tracker {
            let tracker_template = root.join("tracker-template.sqlite");
            if !tracker_template.exists() {
                initialize_tracker_template(&tracker_template)?;
            }
            fs::copy(&tracker_template, &scratch_path)
                .map_err(|error| format!("unable to prepare temporary RTK metrics: {error}"))?;
        }
        let ledger_path = root.join("metrics-v1.sqlite");
        let metrics = Self {
            ledger_path,
            scratch_path,
        };
        if !metrics.ledger_path.exists() {
            metrics.initialize_ledger()?;
        }
        Ok(metrics)
    }

    fn initialize_ledger(&self) -> Result<(), String> {
        let connection = Connection::open(&self.ledger_path)
            .map_err(|error| format!("unable to open local metrics ledger: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA busy_timeout=5000;
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
                 CREATE INDEX IF NOT EXISTS idx_invocations_timestamp ON invocations(timestamp);",
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
        let connection = Connection::open(&self.ledger_path)
            .map_err(|error| format!("unable to reopen local metrics ledger: {error}"))?;
        connection
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
        remove_scratch_database(&self.scratch_path);
        Ok(totals)
    }

    pub(crate) fn print_gain() -> Result<(), String> {
        let root = xuva_data_root();
        let ledger_path = root.join("metrics-v1.sqlite");
        if !ledger_path.exists() {
            println!("XUVA Measured Token Accounting\n\nNo RTK-measured commands yet.");
            return Ok(());
        }
        let connection = Connection::open(&ledger_path)
            .map_err(|error| format!("unable to open local metrics ledger: {error}"))?;
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
}

fn initialize_tracker_template(path: &Path) -> Result<(), String> {
    let connection = Connection::open(path)
        .map_err(|error| format!("unable to create RTK metrics template: {error}"))?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS commands (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                original_cmd TEXT NOT NULL,
                rtk_cmd TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                saved_tokens INTEGER NOT NULL,
                savings_pct REAL NOT NULL,
                exec_time_ms INTEGER DEFAULT 0,
                project_path TEXT DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS idx_timestamp ON commands(timestamp);
             CREATE INDEX IF NOT EXISTS idx_project_path_timestamp ON commands(project_path, timestamp);
             CREATE TABLE IF NOT EXISTS parse_failures (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                raw_command TEXT NOT NULL,
                error_message TEXT NOT NULL,
                fallback_succeeded INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_pf_timestamp ON parse_failures(timestamp);",
        )
        .map_err(|error| format!("unable to initialize RTK metrics template: {error}"))?;
    Ok(())
}

fn read_upstream_totals(path: &Path) -> Result<TokenTotals, String> {
    if !path.exists() {
        return Ok(TokenTotals::default());
    }
    let connection = Connection::open(path)
        .map_err(|error| format!("unable to read temporary RTK metrics: {error}"))?;
    let exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'commands'",
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
        let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
        let _ = fs::remove_file(candidate);
    }
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
