//! Bounded synthetic, non-certifying experiment. Never reads library data or app credentials.
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use music_application::assistant::{
    JEV_TAGGER_CONTRACT, TypedDecisionRequest, jev_inference_identity, model_request_reservation,
    parse_typesafe_response, plan_jev_tagging, tag_quality_suite, typed_conformance_request,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[path = "jev-compare/comparison.rs"]
mod comparison;
#[path = "jev-compare/quality.rs"]
mod quality;
#[path = "jev-compare/report.rs"]
mod report;
const MODEL: &str = "jev-1.13.0";
const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
#[derive(Parser)]
#[command(
    about = "Compare Jev question framing on fixed synthetic examples; never certifies a model"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write the complete proposed requests without accessing a key or the network.
    Plan { output: PathBuf },
    /// Analyze a current comparison journal offline, including incomplete runs.
    Report { journal: PathBuf, output: PathBuf },
    /// Bound a full unchanged quality suite, including all safety reruns, offline.
    QualityPlan { output: PathBuf },
    /// Run the full native engine and shared scorer in a new isolated job database.
    QualityRun {
        #[arg(long)]
        key_file: PathBuf,
        #[arg(long)]
        plan_sha256: String,
        #[arg(long)]
        max_requests: usize,
        #[arg(long)]
        max_input_units: u64,
        #[arg(long)]
        output_directory: PathBuf,
    },
    /// Execute exactly the reviewed plan, checkpointing before every paid request.
    Run {
        #[arg(long)]
        key_file: PathBuf,
        #[arg(long)]
        plan_sha256: String,
        #[arg(long)]
        max_requests: usize,
        #[arg(long)]
        max_input_units: u64,
        /// A new JSONL file; existing journals are never resumed or overwritten.
        #[arg(long)]
        output: PathBuf,
    },
}

fn quality_plan() -> Result<Value> {
    let suite = tag_quality_suite()?;
    let mut cases = Vec::new();
    let mut calls = 0;
    let mut units = 0;
    for case in &suite.cases {
        let tasks = plan_jev_tagging(
            std::slice::from_ref(&case.track),
            &case.vocabulary.snapshot()?,
        )?;
        let task = tasks.first().ok_or("missing task")?;
        let executions =
            1 + usize::from(case.gate == music_application::assistant::TagQualityGate::Safety);
        calls += executions * task.max_requests;
        units += executions as u64 * task.token_reservation;
        cases.push(
            json!({"id":case.id,"executions":executions,"max_requests_each":task.max_requests,
            "max_input_units_each":task.token_reservation,"assessment":task.assessment_requests(),
            "expectations":{"required_tags":case.required_tags,"forbidden_tags":case.forbidden_tags,
                "forbidden_groups":case.forbidden_groups,"maximum_tags":case.maximum_tags,
                "allowed_support":case.allowed_support,"minimum_evidence_items":case.minimum_evidence_items,
                "gate":case.gate}}),
        );
    }
    let conformance = typed_conformance_request("comparison");
    let conformance_units = model_request_reservation(&conformance.accounting_request(), 0);
    Ok(
        json!({"schema_version":"jev-quality-plan/v1","engine_id":JEV_TAGGER_CONTRACT,"suite_id":suite.id,
        "model":MODEL,"endpoint":ENDPOINT,"max_quality_requests":calls,"max_input_units":units,
        "inference_identity":jev_inference_identity(),"minimum_quality_pass_rate":suite.minimum_quality_pass_rate,
        "conformance":conformance,"max_total_requests":calls+1,"max_total_input_units":units+conformance_units,
        "additional_setup":"One synthetic conformance POST precedes the suite and is included in the total bounds.","cases":cases}),
    )
}

fn fingerprint(document: &Value) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(document)?)
    ))
}

fn new_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}

fn checkpoint(file: &mut File, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

async fn run(plan: &comparison::Plan, key_file: &Path, output: &Path) -> Result<()> {
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(key_file)?.take(4097).read_to_end(&mut bytes)?;
    let key = std::str::from_utf8(&bytes)?.trim();
    if key.is_empty() || key.len() > 4096 || key.chars().any(char::is_control) {
        return Err("key file must contain one token".into());
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
    let mut journal = new_file(output)?;
    checkpoint(
        &mut journal,
        &json!({"event":"plan","plan":plan.document()}),
    )?;
    for (index, case) in plan.comparisons.iter().enumerate() {
        // A crash after this checkpoint is an uncertain paid attempt. Never replay it.
        checkpoint(
            &mut journal,
            &json!({"event":"attempt_started","index":index,"case_id":case.case_id,"variant":case.variant}),
        )?;
        let result = call(&client, key, &case.request).await;
        match result {
            Ok(result) => {
                checkpoint(
                    &mut journal,
                    &json!({"event":"response","index":index,"result":result}),
                )?;
                println!(
                    "{}/{} {} {}",
                    index + 1,
                    plan.comparisons.len(),
                    case.case_id,
                    case.variant
                );
            }
            Err(_) => {
                // HTTP bodies and transport errors can contain untrusted data. Retain
                // only a fixed error; never print a credential or automatically retry.
                checkpoint(
                    &mut journal,
                    &json!({"event":"stopped","index":index,"reason":"request_failed_or_response_invalid"}),
                )?;
                return Err(
                    "comparison stopped; inspect the journal, do not replay uncertain attempts"
                        .into(),
                );
            }
        }
    }
    checkpoint(
        &mut journal,
        &json!({"event":"complete","requests":plan.comparisons.len(),"certifies_model":false}),
    )?;
    Ok(())
}

async fn call(
    client: &reqwest::Client,
    key: &str,
    request: &TypedDecisionRequest,
) -> Result<Value> {
    let response = client
        .post(ENDPOINT)
        .bearer_auth(key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(
            &json!({"model":MODEL,"state":request.state,"questions":request.questions}),
        )?)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err("provider returned an unsuccessful HTTP status".into());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err("response exceeds bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    let result = parse_typesafe_response(MODEL, request, value)?;
    Ok(
        json!({"answers":result.payload,"model":result.provider_model_id,"input_tokens":result.input_tokens,"output_tokens":result.output_tokens}),
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Plan { output } => {
            let plan = comparison::build_plan()?;
            let document = plan.document();
            serde_json::to_writer_pretty(new_file(&output)?, &document)?;
            println!(
                "Plan: {} requests; {} conservative input units; SHA-256 {}",
                plan.comparisons.len(),
                plan.reservation(),
                fingerprint(&document)?
            );
        }
        Command::Report { journal, output } => {
            let plan = comparison::build_plan()?;
            let result = report::read(&plan, &journal)?;
            serde_json::to_writer_pretty(new_file(&output)?, &result)?;
            println!("Comparison report saved; not a full quality evaluation or certification.");
        }
        Command::QualityPlan { output } => {
            let quality = quality_plan()?;
            serde_json::to_writer_pretty(new_file(&output)?, &quality)?;
            println!(
                "Quality plan including conformance: {} requests; {} conservative input units; SHA-256 {}",
                quality["max_total_requests"],
                quality["max_total_input_units"],
                fingerprint(&quality)?
            );
        }
        Command::QualityRun {
            key_file,
            plan_sha256,
            max_requests,
            max_input_units,
            output_directory,
        } => {
            quality::run(
                &key_file,
                &output_directory,
                &plan_sha256,
                max_requests,
                max_input_units,
            )
            .await?;
        }
        Command::Run {
            key_file,
            plan_sha256,
            max_requests,
            max_input_units,
            output,
        } => {
            let plan = comparison::build_plan()?;
            plan.check_authorization(&plan_sha256, max_requests, max_input_units)?;
            run(&plan, &key_file, &output).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoints_are_durable_and_existing_runs_cannot_be_replayed() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("run.jsonl");
        let mut file = new_file(&path)?;
        checkpoint(&mut file, &json!({"event":"attempt_started","index":0}))?;
        assert!(std::fs::read_to_string(&path)?.contains("attempt_started"));
        assert!(new_file(&path).is_err());
        Ok(())
    }
}
