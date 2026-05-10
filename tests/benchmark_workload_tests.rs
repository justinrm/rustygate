use std::{fs, path::Path};

use rustygate::{
    config::{AppConfig, RoutingPolicy},
    models::chat::{ChatCompletionRequest, ChatValidationLimits},
    routing::prefix_fingerprint::{fingerprint_request, PrefixFingerprintConfidence},
};

#[test]
fn benchmark_configs_load_with_expected_routing_policies() {
    let priority = AppConfig::from_file(repo_path("benchmarks/configs/mock-priority.toml"))
        .expect("priority benchmark config should load");
    let prefix_affinity =
        AppConfig::from_file(repo_path("benchmarks/configs/mock-prefix-affinity.toml"))
            .expect("prefix-affinity benchmark config should load");

    assert_eq!(priority.gateway.routing_policy, RoutingPolicy::Priority);
    assert_eq!(
        prefix_affinity.gateway.routing_policy,
        RoutingPolicy::PrefixAffinity
    );
    assert_eq!(priority.model_pools[0].members.len(), 3);
    assert_eq!(prefix_affinity.model_pools[0].members.len(), 3);
}

#[test]
fn benchmark_workloads_parse_as_valid_chat_requests() {
    for workload in [
        "shared-prefix",
        "no-shared-prefix",
        "mixed-prompt-lengths",
        "shared-prefix-streaming",
        "no-shared-prefix-streaming",
    ] {
        for request in load_workload(workload) {
            request
                .validate(None, &ChatValidationLimits::default())
                .expect("benchmark workload request should be valid");
        }
    }
}

#[test]
fn shared_prefix_workload_has_stable_high_confidence_fingerprint() {
    let fingerprints = load_workload("shared-prefix")
        .iter()
        .map(fingerprint_request)
        .collect::<Vec<_>>();
    let first = fingerprints[0].fingerprint.clone();

    assert!(fingerprints.iter().all(|fingerprint| {
        fingerprint.confidence == PrefixFingerprintConfidence::High
            && fingerprint.fingerprint == first
    }));
}

#[test]
fn no_shared_prefix_workload_is_low_confidence() {
    let fingerprints = load_workload("no-shared-prefix")
        .iter()
        .map(fingerprint_request)
        .collect::<Vec<_>>();

    assert!(fingerprints.iter().all(|fingerprint| {
        fingerprint.confidence == PrefixFingerprintConfidence::Low
            && fingerprint.fingerprint.is_none()
    }));
}

#[test]
fn streaming_workloads_enable_streaming() {
    for workload in ["shared-prefix-streaming", "no-shared-prefix-streaming"] {
        assert!(load_workload(workload)
            .iter()
            .all(ChatCompletionRequest::stream_enabled));
    }
}

fn load_workload(name: &str) -> Vec<ChatCompletionRequest> {
    let path = repo_path(format!("benchmarks/workloads/{name}.jsonl"));
    fs::read_to_string(&path)
        .expect("workload should be readable")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("workload line should parse"))
        .collect()
}

fn repo_path(path: impl AsRef<Path>) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}
