use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// One buffered heart-rate sample awaiting flush to disk.
struct Sample {
    ts: DateTime<Local>,
    bpm: u16,
}

/// A heart-rate data point returned to the frontend.
#[derive(Serialize)]
pub struct HrPoint {
    /// Unix epoch milliseconds.
    pub t: i64,
    pub bpm: u16,
}

/// Owns the in-memory queue of samples. Samples are pushed by the sampling
/// task and drained to hourly CSV files by the flush task (and on exit).
pub struct Recorder {
    buffer: Mutex<Vec<Sample>>,
}

fn records_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("SpoitableHRS")
        .join("records")
}

/// records/<yyyy>/<mm>/<dd>/<hh>00.csv for the given local timestamp.
fn file_path_for(ts: &DateTime<Local>) -> PathBuf {
    records_dir()
        .join(format!("{:04}", ts.year()))
        .join(format!("{:02}", ts.month()))
        .join(format!("{:02}", ts.day()))
        .join(format!("{:02}00.csv", ts.hour()))
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            buffer: Mutex::new(Vec::new()),
        }
    }

    /// Queue the current heart rate with a local timestamp.
    pub fn record(&self, bpm: u16) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.push(Sample {
                ts: Local::now(),
                bpm,
            });
        }
    }

    /// Drain the queue and append rows to the matching hourly CSV files.
    /// Rows spanning an hour boundary are split into the correct files.
    /// Returns the number of rows written. Safe to call synchronously on exit.
    pub fn flush(&self) -> usize {
        let samples = {
            let Ok(mut buf) = self.buffer.lock() else {
                return 0;
            };
            if buf.is_empty() {
                return 0;
            }
            std::mem::take(&mut *buf)
        };

        let mut groups: BTreeMap<PathBuf, Vec<&Sample>> = BTreeMap::new();
        for s in &samples {
            groups.entry(file_path_for(&s.ts)).or_default().push(s);
        }

        let mut written = 0;
        for (path, rows) in groups {
            if append_rows(&path, &rows).is_ok() {
                written += rows.len();
            }
        }
        written
    }
}

fn append_rows(path: &PathBuf, rows: &[&Sample]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let new_file = !path.exists();
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    if new_file {
        writeln!(f, "timestamp,bpm")?;
    }
    for s in rows {
        writeln!(f, "{},{}", s.ts.to_rfc3339(), s.bpm)?;
    }
    Ok(())
}

/// Read all HR points with timestamps in [from_ms, to_ms] (inclusive).
pub fn read_range(from_ms: i64, to_ms: i64) -> Vec<HrPoint> {
    let mut out = Vec::new();
    if to_ms < from_ms {
        return out;
    }
    let Some(from) = Local.timestamp_millis_opt(from_ms).single() else {
        return out;
    };
    let Some(to) = Local.timestamp_millis_opt(to_ms).single() else {
        return out;
    };

    // Walk each hour bucket from the start hour through the end.
    let mut cursor = from
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(from);

    while cursor <= to {
        let path = file_path_for(&cursor);
        if let Ok(content) = fs::read_to_string(&path) {
            for line in content.lines().skip(1) {
                let mut parts = line.splitn(2, ',');
                let (Some(ts_str), Some(bpm_str)) = (parts.next(), parts.next()) else {
                    continue;
                };
                let Ok(ts) = DateTime::parse_from_rfc3339(ts_str) else {
                    continue;
                };
                let t = ts.timestamp_millis();
                if t < from_ms || t > to_ms {
                    continue;
                }
                if let Ok(bpm) = bpm_str.trim().parse::<u16>() {
                    out.push(HrPoint { t, bpm });
                }
            }
        }
        cursor += chrono::Duration::hours(1);
    }

    out.sort_by_key(|p| p.t);
    out
}

/// The records root directory, created if missing.
pub fn records_root() -> PathBuf {
    let dir = records_dir();
    fs::create_dir_all(&dir).ok();
    dir
}
