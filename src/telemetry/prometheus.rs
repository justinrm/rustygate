use crate::telemetry::metrics::MetricsSnapshot;

pub fn render_prometheus(snapshot: &MetricsSnapshot) -> String {
    let mut output = String::new();

    write_metric(
        &mut output,
        "rustygate_requests_total",
        "Total chat requests handled by the gateway.",
        "counter",
        snapshot.total_requests as f64,
    );
    write_metric(
        &mut output,
        "rustygate_requests_success_total",
        "Total successful chat requests handled by the gateway.",
        "counter",
        snapshot.successful_requests as f64,
    );
    write_metric(
        &mut output,
        "rustygate_requests_failed_total",
        "Total failed chat requests handled by the gateway.",
        "counter",
        snapshot.failed_requests as f64,
    );
    write_metric(
        &mut output,
        "rustygate_in_flight_requests",
        "Current in-flight chat requests.",
        "gauge",
        snapshot.in_flight_requests as f64,
    );
    write_metric(
        &mut output,
        "rustygate_request_error_rate",
        "Ratio of failed chat requests to total chat requests.",
        "gauge",
        snapshot.error_rate,
    );
    write_metric(
        &mut output,
        "rustygate_request_latency_ms_avg",
        "Average chat request latency in milliseconds.",
        "gauge",
        snapshot.avg_latency_ms,
    );
    write_metric(
        &mut output,
        "rustygate_request_latency_ms_p95",
        "P95 chat request latency in milliseconds.",
        "gauge",
        snapshot.p95_latency_ms,
    );
    write_metric(
        &mut output,
        "rustygate_provider_attempts_total",
        "Total provider attempts made by the gateway.",
        "counter",
        snapshot.total_provider_attempts as f64,
    );
    write_metric(
        &mut output,
        "rustygate_fallback_attempts_total",
        "Total fallback provider attempts made by the gateway.",
        "counter",
        snapshot.fallback_attempts as f64,
    );
    write_metric(
        &mut output,
        "rustygate_estimated_tokens_total",
        "Total estimated tokens processed by the gateway.",
        "counter",
        snapshot.estimated_total_tokens as f64,
    );
    write_metric(
        &mut output,
        "rustygate_estimated_cost_usd_total",
        "Total estimated token cost in USD.",
        "counter",
        snapshot.estimated_total_cost_usd,
    );

    write_labeled_header(
        &mut output,
        "rustygate_request_errors_total",
        "Chat request errors by category.",
        "counter",
    );
    for (category, count) in &snapshot.request_errors_by_category {
        write_labeled_metric(
            &mut output,
            "rustygate_request_errors_total",
            &[("category", category)],
            *count as f64,
        );
    }

    write_labeled_header(
        &mut output,
        "rustygate_provider_requests_total",
        "Provider attempts by provider.",
        "counter",
    );
    for (provider, count) in &snapshot.requests_by_provider {
        write_labeled_metric(
            &mut output,
            "rustygate_provider_requests_total",
            &[("provider", provider)],
            *count as f64,
        );
    }

    write_labeled_header(
        &mut output,
        "rustygate_provider_successes_total",
        "Provider successful attempts by provider.",
        "counter",
    );
    for (provider, count) in &snapshot.successes_by_provider {
        write_labeled_metric(
            &mut output,
            "rustygate_provider_successes_total",
            &[("provider", provider)],
            *count as f64,
        );
    }

    write_labeled_header(
        &mut output,
        "rustygate_provider_errors_total",
        "Provider failed attempts by provider and category.",
        "counter",
    );
    for (provider, categories) in &snapshot.provider_errors_by_provider_and_category {
        for (category, count) in categories {
            write_labeled_metric(
                &mut output,
                "rustygate_provider_errors_total",
                &[("provider", provider), ("category", category)],
                *count as f64,
            );
        }
    }

    write_labeled_header(
        &mut output,
        "rustygate_provider_latency_ms_avg",
        "Average provider attempt latency in milliseconds.",
        "gauge",
    );
    for (provider, latency_ms) in &snapshot.avg_latency_ms_by_provider {
        write_labeled_metric(
            &mut output,
            "rustygate_provider_latency_ms_avg",
            &[("provider", provider)],
            *latency_ms,
        );
    }

    write_labeled_header(
        &mut output,
        "rustygate_provider_latency_ms_p95",
        "P95 provider attempt latency in milliseconds.",
        "gauge",
    );
    for (provider, latency_ms) in &snapshot.p95_latency_ms_by_provider {
        write_labeled_metric(
            &mut output,
            "rustygate_provider_latency_ms_p95",
            &[("provider", provider)],
            *latency_ms,
        );
    }

    output
}

fn write_metric(output: &mut String, name: &str, help: &str, kind: &str, value: f64) {
    write_labeled_header(output, name, help, kind);
    output.push_str(name);
    output.push(' ');
    output.push_str(&format_prometheus_number(value));
    output.push('\n');
}

fn write_labeled_header(output: &mut String, name: &str, help: &str, kind: &str) {
    output.push_str("# HELP ");
    output.push_str(name);
    output.push(' ');
    output.push_str(help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(name);
    output.push(' ');
    output.push_str(kind);
    output.push('\n');
}

fn write_labeled_metric(output: &mut String, name: &str, labels: &[(&str, &str)], value: f64) {
    output.push_str(name);
    output.push('{');
    for (index, (key, value)) in labels.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(key);
        output.push_str("=\"");
        output.push_str(&escape_label_value(value));
        output.push('"');
    }
    output.push_str("} ");
    output.push_str(&format_prometheus_number(value));
    output.push('\n');
}

fn escape_label_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

fn format_prometheus_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::render_prometheus;
    use crate::telemetry::metrics::MetricsSnapshot;

    #[test]
    fn renders_core_and_labeled_metrics() {
        let mut snapshot = MetricsSnapshot {
            total_requests: 2,
            successful_requests: 1,
            failed_requests: 1,
            in_flight_requests: 3,
            error_rate: 0.5,
            avg_latency_ms: 12.5,
            p95_latency_ms: 20.0,
            total_provider_attempts: 2,
            ..Default::default()
        };
        snapshot
            .request_errors_by_category
            .insert("timeout".into(), 1);
        snapshot
            .provider_errors_by_provider_and_category
            .entry("mock-fast".into())
            .or_default()
            .insert("rate_limited".into(), 1);

        let rendered = render_prometheus(&snapshot);

        assert!(rendered.contains("rustygate_requests_total 2\n"));
        assert!(rendered.contains("rustygate_in_flight_requests 3\n"));
        assert!(rendered.contains("rustygate_request_errors_total{category=\"timeout\"} 1\n"));
        assert!(rendered.contains(
            "rustygate_provider_errors_total{provider=\"mock-fast\",category=\"rate_limited\"} 1\n"
        ));
    }
}
