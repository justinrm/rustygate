use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use clap::Parser;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use tokio::{sync::Mutex, task::JoinSet};

#[derive(Debug, Parser)]
#[command(name = "rustygate_benchmark")]
#[command(about = "Run lightweight RustyGate HTTP benchmark workloads")]
struct Cli {
    #[arg(long)]
    url: String,
    #[arg(long)]
    workload: PathBuf,
    #[arg(long, default_value = "benchmark-key")]
    api_key: String,
    #[arg(long, default_value_t = 30)]
    duration_seconds: u64,
    #[arg(long, default_value_t = 50)]
    concurrency: usize,
    #[arg(long)]
    stats_url: Option<String>,
    #[arg(long)]
    provider_stats_url: Option<String>,
    #[arg(long)]
    metrics_url: Option<String>,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    url: String,
    workload: String,
    duration_seconds: u64,
    concurrency: usize,
    elapsed_ms: u128,
    total_requests: usize,
    successful_requests: usize,
    failed_requests: usize,
    requests_per_second: f64,
    status_codes: BTreeMap<String, usize>,
    client_error_count: usize,
    latency_ms: PercentileSummary,
    ttft_ms: Option<PercentileSummary>,
    prefix_affinity_hit_rate: Option<f64>,
    stats: Option<Value>,
    provider_stats: Option<Value>,
    prometheus_metrics: Option<String>,
}

#[derive(Debug, Serialize)]
struct PercentileSummary {
    p50: f64,
    p95: f64,
    p99: f64,
}

#[derive(Debug)]
struct RequestOutcome {
    status: Option<u16>,
    latency_ms: u64,
    ttft_ms: Option<u64>,
    error: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    anyhow::ensure!(cli.concurrency > 0, "--concurrency must be greater than 0");
    anyhow::ensure!(
        cli.duration_seconds > 0,
        "--duration-seconds must be greater than 0"
    );

    let workload = Arc::new(load_workload(&cli.workload)?);
    anyhow::ensure!(
        !workload.is_empty(),
        "workload must contain at least one request"
    );

    let client = Client::builder()
        .timeout(Duration::from_secs(cli.duration_seconds.saturating_add(30)))
        .build()?;
    let next_request = Arc::new(AtomicUsize::new(0));
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let started = Instant::now();
    let deadline = started + Duration::from_secs(cli.duration_seconds);

    let mut tasks = JoinSet::new();
    for _ in 0..cli.concurrency {
        let client = client.clone();
        let url = cli.url.clone();
        let api_key = cli.api_key.clone();
        let workload = Arc::clone(&workload);
        let next_request = Arc::clone(&next_request);
        let outcomes = Arc::clone(&outcomes);
        tasks.spawn(async move {
            while Instant::now() < deadline {
                let index = next_request.fetch_add(1, Ordering::Relaxed) % workload.len();
                let outcome = run_one_request(&client, &url, &api_key, &workload[index]).await;
                outcomes.lock().await.push(outcome);
            }
        });
    }

    while let Some(result) = tasks.join_next().await {
        result?;
    }

    let elapsed = started.elapsed();
    let outcomes = outcomes.lock().await;
    let mut status_codes = BTreeMap::new();
    let mut successful_requests = 0;
    let mut failed_requests = 0;
    let mut client_error_count = 0;
    let mut latencies = Vec::with_capacity(outcomes.len());
    let mut ttfts = Vec::new();

    for outcome in outcomes.iter() {
        latencies.push(outcome.latency_ms);
        if let Some(ttft_ms) = outcome.ttft_ms {
            ttfts.push(ttft_ms);
        }
        if outcome.error {
            client_error_count += 1;
            failed_requests += 1;
            continue;
        }
        match outcome.status {
            Some(status) if (200..300).contains(&status) => {
                successful_requests += 1;
                *status_codes.entry(status.to_string()).or_default() += 1;
            }
            Some(status) => {
                failed_requests += 1;
                *status_codes.entry(status.to_string()).or_default() += 1;
            }
            None => {
                client_error_count += 1;
                failed_requests += 1;
            }
        }
    }

    let stats = fetch_json(&client, cli.stats_url.as_deref(), &cli.api_key).await;
    let provider_stats = fetch_json(&client, cli.provider_stats_url.as_deref(), &cli.api_key).await;
    let prometheus_metrics = fetch_text(&client, cli.metrics_url.as_deref(), &cli.api_key).await;
    let prefix_affinity_hit_rate = stats.as_ref().and_then(prefix_affinity_hit_rate);

    let report = BenchmarkReport {
        url: cli.url,
        workload: cli.workload.display().to_string(),
        duration_seconds: cli.duration_seconds,
        concurrency: cli.concurrency,
        elapsed_ms: elapsed.as_millis(),
        total_requests: outcomes.len(),
        successful_requests,
        failed_requests,
        requests_per_second: outcomes.len() as f64 / elapsed.as_secs_f64(),
        status_codes,
        client_error_count,
        latency_ms: percentile_summary(&latencies),
        ttft_ms: if ttfts.is_empty() {
            None
        } else {
            Some(percentile_summary(&ttfts))
        },
        prefix_affinity_hit_rate,
        stats,
        provider_stats,
        prometheus_metrics,
    };

    let encoded = serde_json::to_string_pretty(&report)?;
    if let Some(output) = cli.output {
        fs::write(output, encoded)?;
    } else {
        println!("{encoded}");
    }

    Ok(())
}

fn load_workload(path: &PathBuf) -> anyhow::Result<Vec<Value>> {
    let contents = fs::read_to_string(path)?;
    contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.trim();
            if line.is_empty() {
                None
            } else {
                Some(
                    serde_json::from_str::<Value>(line).map_err(|error| {
                        anyhow::anyhow!("{}:{}: {error}", path.display(), index + 1)
                    }),
                )
            }
        })
        .collect()
}

async fn run_one_request(
    client: &Client,
    url: &str,
    api_key: &str,
    body: &Value,
) -> RequestOutcome {
    let started = Instant::now();
    let stream_enabled = body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or_default();

    let response = match client
        .post(url)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return RequestOutcome {
                status: None,
                latency_ms: started.elapsed().as_millis() as u64,
                ttft_ms: None,
                error: true,
            };
        }
    };

    let status = response.status().as_u16();
    let mut ttft_ms = None;
    let body_result = if stream_enabled && response.status().is_success() {
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if ttft_ms.is_none() && !bytes.is_empty() {
                        ttft_ms = Some(started.elapsed().as_millis() as u64);
                    }
                }
                Err(error) => return request_error(started, Some(status), ttft_ms, error),
            }
        }
        Ok(())
    } else {
        response.bytes().await.map(|_| ())
    };

    match body_result {
        Ok(()) => RequestOutcome {
            status: Some(status),
            latency_ms: started.elapsed().as_millis() as u64,
            ttft_ms,
            error: false,
        },
        Err(error) => request_error(started, Some(status), ttft_ms, error),
    }
}

fn request_error(
    started: Instant,
    status: Option<u16>,
    ttft_ms: Option<u64>,
    _error: reqwest::Error,
) -> RequestOutcome {
    RequestOutcome {
        status,
        latency_ms: started.elapsed().as_millis() as u64,
        ttft_ms,
        error: true,
    }
}

async fn fetch_json(client: &Client, url: Option<&str>, api_key: &str) -> Option<Value> {
    let url = url?;
    client
        .get(url)
        .bearer_auth(api_key)
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()
}

async fn fetch_text(client: &Client, url: Option<&str>, api_key: &str) -> Option<String> {
    let url = url?;
    client
        .get(url)
        .bearer_auth(api_key)
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()
}

fn percentile_summary(samples: &[u64]) -> PercentileSummary {
    PercentileSummary {
        p50: percentile(samples, 0.50),
        p95: percentile(samples, 0.95),
        p99: percentile(samples, 0.99),
    }
}

fn percentile(samples: &[u64], percentile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let percentile_index = ((sorted.len() as f64) * percentile).ceil() as usize - 1;
    sorted[percentile_index] as f64
}

fn prefix_affinity_hit_rate(stats: &Value) -> Option<f64> {
    let prefix_affinity = stats
        .get("routing_decisions_by_policy_and_reason")?
        .get("prefix_affinity")?;
    let hits = prefix_affinity
        .get("prefix_hit")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let misses = prefix_affinity
        .get("prefix_miss")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total = hits + misses;
    if total == 0 {
        None
    } else {
        Some(hits as f64 / total as f64)
    }
}
