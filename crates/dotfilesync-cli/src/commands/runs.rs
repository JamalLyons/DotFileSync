//! `dotfilesync runs` — list and show apply runs kept by the daemon.

use anyhow::Result;
use clap::Args;
use dotfilesync_protocol::RunView;
use std::path::Path;

use super::mode;
use crate::output::json;

#[derive(Debug, Args)]
pub struct RunsArgs {
    #[command(subcommand)]
    pub command: Option<RunsCommand>,

    /// With no subcommand, print JSON instead of a table
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, clap::Subcommand)]
pub enum RunsCommand {
    /// List runs, including interrupted runs from a previous daemon process
    List,
    /// Show one run's journal
    Show { run_id: String },
}

pub fn execute(args: RunsArgs, data_dir: Option<&Path>) -> Result<()> {
    let mut client = mode::connect(data_dir)?;
    match args.command {
        None | Some(RunsCommand::List) => {
            let runs = client.plan_runs()?;
            if args.json {
                println!("{}", json::render_json(&runs)?);
            } else if runs.is_empty() {
                println!("No runs.");
            } else {
                for run in &runs {
                    println!(
                        "{}  {:<12}  plan {}  job {}",
                        run.run_id, run.status, run.plan_id, run.job_id
                    );
                }
            }
        }
        Some(RunsCommand::Show { run_id }) => {
            let view = client.plan_run_status(&run_id)?;
            if args.json {
                println!("{}", json::render_json(&view)?);
            } else {
                print!("{}", render_view(&view));
            }
        }
    }
    Ok(())
}

fn render_view(view: &RunView) -> String {
    let mut out = format!(
        "Run {}  {}  plan {}  job {}\n",
        view.run_id, view.status, view.plan_id, view.job_id
    );
    if view.steps.is_empty() {
        out.push_str("No steps recorded.\n");
        return out;
    }
    for step in &view.steps {
        out.push_str(&format!("  {}  {:?}\n", step.action_id, step.status));
        if let Some(error) = &step.error {
            out.push_str(&format!("    {error}\n"));
        }
    }
    out
}
