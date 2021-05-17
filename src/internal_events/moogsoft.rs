use super::InternalEvent;
use metrics::counter;

#[derive(Debug)]
pub struct MoogsoftMetricEncoded {
    pub byte_size: usize,
}

impl InternalEvent for MoogsoftMetricEncoded {
    fn emit_logs(&self) {
        trace!(message = "Moogsoft metric encoded.");
    }

    fn emit_metrics(&self) {
        counter!("processed_bytes_total", self.byte_size as u64);
    }
}

#[derive(Debug)]
pub struct LogEventDiscarded;

impl InternalEvent for LogEventDiscarded {
    fn emit_logs(&self) {
        trace!(message = "LogEvent could not be converted to Moogsoft metric.");
    }

    fn emit_metrics(&self) {
        counter!("events_discarded_total", 1);
    }
}

#[derive(Debug)]
pub struct MetricDiscarded;

impl InternalEvent for MetricDiscarded {
    fn emit_logs(&self) {
        trace!(message = "Metric could not be converted to Moogsoft metric.");
    }

    fn emit_metrics(&self) {
        counter!("events_discarded_total", 1);
    }
}
