# Benchmarks

RustyGate includes a reproducible benchmark harness in `benchmarks/`.

## Methodology

The harness compares RustyGate with a Python LiteLLM proxy using the same OpenAI-shaped request payload and local mock model path. It is intended to measure gateway overhead, not real LLM latency.

Run:

```sh
./benchmarks/run.sh
```

The script starts Docker Compose, sends 30 seconds of POST traffic with `oha`, and samples resident memory once per second. Raw output is written under `benchmarks/results/`.

## Reported Metrics

Each run should record:

- Requests per second.
- p50, p95, and p99 latency from `oha`.
- Idle RSS and sustained-load RSS from `record-rss.sh`.
- Container image size from `docker images`.

## Baseline Results

Results pending: run `./benchmarks/run.sh` on the target machine and commit the raw `benchmarks/results/` output alongside a short machine profile before using this page as a portfolio artifact. Treat the checked-in harness as the source of truth and avoid overclaiming: both gateways are mostly forwarding to deterministic local mock behavior, so the benchmark is about proxy overhead and memory footprint.

## Caveats

LiteLLM and RustyGate do not expose identical feature sets. This benchmark is intentionally narrow: one OpenAI-compatible chat request, local network, fixed concurrency, and no real provider latency. It is useful for comparing gateway overhead, not for predicting end-user model latency.
