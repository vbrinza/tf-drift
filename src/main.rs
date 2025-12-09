use clap::Parser;
use std::{path::PathBuf, process::Stdio};
use tokio::{process::Command, task::JoinSet};
// use tokio::process::Command;
use walkdir::WalkDir;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    #[arg(short, long)]
    path: String,
    #[arg(short, long)]
    max_concurency: usize,
}

#[derive(Debug, Clone)]
pub enum PlanStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone)]
struct PlanResult {
    pub path: String,
    pub status: PlanStatus,
    pub changes_count: u32,
    pub stdout: String,
    pub stderr: String,
    pub plan_file: String,
    pub error: Option<String>,
}

pub fn find_tg_dirs(path: &str) -> Vec<PathBuf> {
    let mut tg_dirs = Vec::new();

    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        let f_name = entry.file_name().to_string_lossy();
        if f_name.ends_with(".hcl") {
            if let Some(parent) = entry.path().parent() {
                tg_dirs.push(parent.to_path_buf());
            }
        }
    }
    println!("Found {} terragrunt dirs", tg_dirs.len());
    tg_dirs
}

async fn run_terragrunt_plan(path: PathBuf) -> PlanResult {
    let plan_file = path.join("plan.tfplan");
    let path_string = path.to_string_lossy().to_string();

    let output = Command::new("terragrunt")
        .arg("plan")
        .arg("-out")
        .arg(&plan_file)
        .current_dir(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                let changes_count = parse_plan_changes(&stdout);

                if changes_count > 0 {
                    println!("Drift in {}: {} changes", path_string, changes_count);
                } else {
                    println!("No drift in {}", path_string);
                }

                PlanResult {
                    path: path_string,
                    status: PlanStatus::Success,
                    changes_count,
                    stdout,
                    stderr,
                    plan_file: plan_file.to_string_lossy().to_string(),
                    error: None,
                }
            } else {
                println!("Plan failed for {}", path_string);
                PlanResult {
                    path: path_string,
                    status: PlanStatus::Failed,
                    changes_count: 0,
                    stdout,
                    stderr: stderr.clone(),
                    plan_file: String::new(),
                    error: Some(stderr),
                }
            }
        }
        Err(e) => {
            println!("Failed to execute command for {}: {}", path_string, e);
            PlanResult {
                path: path_string,
                status: PlanStatus::Failed,
                changes_count: 0,
                stdout: String::new(),
                stderr: String::new(),
                plan_file: String::new(),
                error: Some(format!("Failed to execute: {}", e)),
            }
        }
    }
}

fn parse_plan_changes(output: &str) -> u32 {
    if output.contains("No changes") {
        return 0;
    }

    for line in output.lines() {
        if line.contains("Plan:") {
            let numbers: Vec<u32> = line
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            return numbers.iter().sum();
        }
    }

    0
}

pub async fn run_plans(dirs: Vec<PathBuf>, max_concurency: usize) -> Vec<PlanResult> {
    let mut tasks = JoinSet::new();
    let mut results = Vec::new();
    let mut count = 0;

    for dir in dirs {
        tasks.spawn(run_terragrunt_plan(dir));
        count += 1;

        if count >= max_concurency {
            if let Some(result) = tasks.join_next().await {
                if let Ok(plan_result) = result {
                    results.push(plan_result);
                }
            }
        }
    }

    while let Some(result) = tasks.join_next().await {
        if let Ok(plan_result) = result {
            results.push(plan_result);
        }
    }

    results
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let tg_dirs = find_tg_dirs(&args.path);
    let max_concurency = args.max_concurency;
    let results = run_plans(tg_dirs, max_concurency).await;
    for result in results {
        match result.status {
            PlanStatus::Success => {
                if result.changes_count > 0 {
                    println!("Store drift: {} -> {}", result.path, result.changes_count);
                }
            }
            PlanStatus::Failed => {
                println!("Store error: {} -> {:?}", result.path, result.error);
            }
        }
    }
}
