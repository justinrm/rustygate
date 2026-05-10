# Benchmarks

RustyGate includes a reproducible benchmark harness in `benchmarks/`.

## Methodology

The default harness exercises RustyGate with local deterministic mock replicas. It is intended to measure gateway overhead, admission-control overhead, prefix fingerprinting overhead, and routing behavior. It does not measure real LLM latency or prove GPU KV-cache reuse.

Run:

```sh
./benchmarks/run.sh
```

The script builds the lightweight `rustygate_benchmark` client, starts Docker Compose with benchmark-specific config, runs each workload for 30 seconds at 50 concurrent requests, samples resident memory once per second, and writes raw output under `benchmarks/results/`.

You can override the defaults:

```sh
DURATION_SECONDS=10 CONCURRENCY=20 ./benchmarks/run.sh
```

Each scenario restarts RustyGate so the stats snapshot belongs to one workload/config pair. The harness runs both `mock-priority` and `mock-prefix-affinity` configs against:

- `shared-prefix`: repeated stable system prompt with different final user messages.
- `no-shared-prefix`: user-only prompts that should not build high-confidence prefix affinity.
- `mixed-prompt-lengths`: short, medium, and longer prompts to expose prompt-size overhead.
- `shared-prefix-streaming`: shared-prefix requests with `stream: true`.
- `no-shared-prefix-streaming`: user-only streaming requests.

## Reported Metrics

Each run should record:

- Requests per second.
- p50, p95, and p99 latency from the Rust benchmark client.
- TTFT p50 and p95 for streaming workloads.
- Admission rejections from `/stats`.
- Prefix-affinity hit rate from routing-decision counters.
- Provider request distribution and in-flight distribution from `/stats/providers`.
- Raw Prometheus text from `/metrics`, including routing, prefix, admission, TTFT, and provider metrics.
- Idle RSS and sustained-load RSS from `record-rss.sh`.
- Container image size from `docker images`.

Raw JSON result files include the post-run `/stats`, `/stats/providers`, and `/metrics` payloads. Keep those raw files when publishing benchmark results so reviewers can inspect the counters behind any summary.

## Baseline Results

Results pending: run `./benchmarks/run.sh` on the target machine and commit the raw `benchmarks/results/` output alongside a short machine profile before using this page as a portfolio artifact.

For a useful machine profile, record CPU model, core count, memory, OS, Docker version, Rust version, `DURATION_SECONDS`, and `CONCURRENCY`. Treat the checked-in harness as the source of truth and avoid overclaiming: the mock-provider benchmark is about gateway overhead, memory footprint, and whether prefix-aware routing behaves as expected under synthetic traffic.

## Optional Self-Hosted Runtime Benchmark

To measure whether prefix-aware routing helps real inference serving, point a model pool at local OpenAI-compatible workers such as vLLM instances. Keep this separate from default CI because it depends on hardware, model choice, runtime version, and runtime flags.

Recommended procedure:

1. Start two or more local OpenAI-compatible runtime workers serving the same model on separate ports.
2. Create a benchmark config under `benchmarks/configs/` with `kind = "openai_compatible"` providers, a shared model pool, and `routing_policy = "prefix_affinity"`.
3. Use the same public model ID in the JSONL workloads.
4. Run the Rust benchmark client against RustyGate and save raw JSON results under `benchmarks/results/`.
5. Record model name, quantization if any, GPU model, GPU memory, runtime version, runtime flags, batch/concurrency settings, and whether runtime prefix caching was enabled.

When available, also save runtime metrics for queue depth, prefix-cache hit rate, KV-cache utilization, and model-server TTFT. Those runtime metrics are the only evidence that the model server reused prefix/KV state; RustyGate's own prefix-affinity counters only show gateway routing decisions.

## Caveats

Mock replicas return deterministic local responses and do not perform prefill, decode, batching, or KV-cache allocation. A shared-prefix mock run can show when RustyGate sticks repeated prefixes to the same healthy replica and what overhead the gateway adds, but it cannot prove model-runtime speedups.

No-shared-prefix workloads should remain close to the priority baseline. If they regress substantially, investigate fingerprinting, admission checks, request validation, or benchmark client overhead before attributing the change to inference behavior.
