use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{sync_file, ApplyError, StepRecord};
use crate::plan::Plan;

pub struct JournalStore {
    path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum JournalLine {
    Header { run_id: String, plan_digest: String },
    Step(StepRecord),
}

impl JournalStore {
    pub fn create(
        run_dir: &Path,
        run_id: &str,
        plan_digest: &str,
        plan: &Plan,
    ) -> Result<Self, ApplyError> {
        let plan_path = run_dir.join("plan.json");
        let body = serde_json::to_vec_pretty(plan)
            .map_err(|error| ApplyError::Journal(format!("failed to encode plan: {error}")))?;
        sync_file(&plan_path, &body).map_err(ApplyError::Journal)?;
        let path = run_dir.join("journal.jsonl");
        if path.exists() {
            return Err(ApplyError::Journal(format!(
                "journal already exists at '{}'; resume it instead of starting a new run",
                path.display()
            )));
        }
        let store = Self { path };
        store.append(&JournalLine::Header {
            run_id: run_id.to_string(),
            plan_digest: plan_digest.to_string(),
        })?;
        Ok(store)
    }

    pub fn open(run_dir: &Path) -> Result<Self, ApplyError> {
        let path = run_dir.join("journal.jsonl");
        if !path.is_file() {
            return Err(ApplyError::Journal(format!(
                "journal not found at '{}'",
                path.display()
            )));
        }
        Ok(Self { path })
    }

    pub fn load(&self) -> Result<(String, String, Vec<StepRecord>), ApplyError> {
        let file = fs::File::open(&self.path)
            .map_err(|error| ApplyError::Journal(format!("failed to read journal: {error}")))?;
        let mut run_id = None;
        let mut plan_digest = None;
        let mut steps = Vec::new();
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|error| {
                ApplyError::Journal(format!("failed to read journal line: {error}"))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let parsed: JournalLine = serde_json::from_str(&line).map_err(|error| {
                ApplyError::Journal(format!("invalid journal line {}: {error}", index + 1))
            })?;
            match parsed {
                JournalLine::Header {
                    run_id: id,
                    plan_digest: digest,
                } => {
                    run_id = Some(id);
                    plan_digest = Some(digest);
                }
                JournalLine::Step(step) => steps.push(step),
            }
        }
        let run_id =
            run_id.ok_or_else(|| ApplyError::Journal("journal is missing a header".into()))?;
        let plan_digest = plan_digest
            .ok_or_else(|| ApplyError::Journal("journal is missing a plan digest".into()))?;
        Ok((run_id, plan_digest, steps))
    }

    pub fn append_step(&self, step: &StepRecord) -> Result<(), ApplyError> {
        self.append(&JournalLine::Step(step.clone()))
    }

    fn append(&self, line: &JournalLine) -> Result<(), ApplyError> {
        let encoded = serde_json::to_string(line)
            .map_err(|error| ApplyError::Journal(format!("failed to encode journal: {error}")))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| ApplyError::Journal(format!("failed to open journal: {error}")))?;
        writeln!(file, "{encoded}")
            .map_err(|error| ApplyError::Journal(format!("failed to append journal: {error}")))?;
        file.flush()
            .map_err(|error| ApplyError::Journal(format!("failed to flush journal: {error}")))?;
        file.sync_all()
            .map_err(|error| ApplyError::Journal(format!("failed to fsync journal: {error}")))?;
        Ok(())
    }
}
