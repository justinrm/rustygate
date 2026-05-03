# OpenTelemetry Demo Trace

This demo starts RustyGate with OTLP trace export enabled and sends a request that exercises provider retries and fallback. The exported trace should show:

- A root RustyGate request span for `/v1/chat/completions`.
- Child spans for each provider attempt.
- Retry and fallback metadata on provider-attempt spans.
- Stream lifecycle events when the request uses `stream: true`.

Run from the repository root:

```sh
docs/observability/otel-demo/run-demo.sh
```

Open Jaeger at `http://127.0.0.1:16686` and select the `rustygate` service. The script writes a real export to `latest-trace.json`. `sample-trace.example.json` is a synthetic fixture that documents the expected span shape; it is not a Jaeger export.
