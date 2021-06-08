use crate::event::metric::MetricTags;
use crate::event::{LogEvent, Metric, MetricKind, Value};
use crate::internal_events::{LogEventDiscarded, MetricDiscarded, MoogsoftMetricEncoded};
use crate::{
    config::{log_schema, DataType, SinkConfig, SinkContext, SinkDescription},
    event::Event,
    event::MetricValue::Gauge,
    http::HttpClient,
    sinks::util::{
        http::{BatchedHttpSink, HttpSink, RequestConfig},
        BatchConfig, BatchSettings, Buffer, Compression, Concurrency, EncodedEvent,
        TowerRequestConfig, UriSerde,
    },
};
use chrono::Utc;
use futures::{future, FutureExt, SinkExt};
use http::{uri, Method, Request, Uri};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MoogsoftSinkConfig {
    pub api_key: String,
    pub uri: UriSerde,
}

impl Default for MoogsoftSinkConfig {
    fn default() -> Self {
        MoogsoftSinkConfig {
            api_key: default_api_key(),
            uri: default_uri(),
        }
    }
}

// We are currently limited by the server side metric format
// Currently no mapping for: class, utc_offset, uuid, fqm,
// detector_type and unit
#[derive(Deserialize, Serialize, Debug, Default)]
struct MoogsoftMetric {
    metric: String,
    key: String,
    source: String,
    data: f64,
    time: i64,
    tags: MetricTags,
}

lazy_static! {
    // These might not be the best defaults for MOC
    // but we can evolve this as needed
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        concurrency: Concurrency::Fixed(10),
        timeout_secs: Some(30),
        rate_limit_num: Some(u64::max_value()),
        ..Default::default()
    };
}

inventory::submit! {
    SinkDescription::new::<MoogsoftSinkConfig>("moogsoft")
}

impl_generate_config_from_default!(MoogsoftSinkConfig);

fn default_api_key() -> String {
    String::new()
}

fn default_uri() -> UriSerde {
    uri_for_authority("api.moogsoft.ai")
}

fn uri_for_authority(authority: &str) -> UriSerde {
    let uri = uri::Builder::new()
        .scheme("http")
        .authority(authority)
        .path_and_query("/v1/integrations/metrics")
        .build()
        .unwrap();
    UriSerde::from(uri)
}

fn get_hostname() -> Option<String> {
    crate::get_hostname().ok()
}

const METRIC: &str = "metric";
const DATA: &str = "data";
const KEY: &str = "key";
const SOURCE: &str = "source";
const TIME: &str = "time";

impl MoogsoftMetric {
    fn with_metric(&mut self, metric: String) -> &Self {
        self.metric = metric;
        self
    }

    fn with_data(&mut self, data: f64) -> &Self {
        self.data = data;
        self
    }

    fn with_key(&mut self, key: String) -> &Self {
        self.key = key;
        self
    }

    fn with_source(&mut self, source: String) -> &Self {
        self.source = source;
        self
    }

    fn with_time(&mut self, time: i64) -> &Self {
        self.time = time;
        self
    }

    fn with_tags(&mut self, tags: MetricTags) -> &Self {
        self.tags = tags;
        self
    }
}

impl TryFrom<LogEvent> for MoogsoftMetric {
    type Error = ();

    fn try_from(log: LogEvent) -> Result<Self, Self::Error> {
        let mut moogsoft_metric: MoogsoftMetric = Default::default();
        let mut is_valid = true;

        if let Some(value) = log.get(METRIC) {
            moogsoft_metric.with_metric(value.to_string_lossy());
        } else {
            trace!("LogEvent cannot be converted as it is missing a metric name.");
            is_valid = false;
        }

        if is_valid {
            if let Some(value) = log.get(DATA) {
                match value {
                    Value::Integer(data) => {
                        moogsoft_metric.with_data(*data as f64);
                    }
                    Value::Float(data) => {
                        moogsoft_metric.with_data(*data);
                    }
                    _ => {
                        trace!("LogEvent cannot be converted as it has invalid data type.");
                        is_valid = false;
                    }
                }
            } else {
                trace!("LogEvent cannot be converted as it is missing a data value.");
                is_valid = false;
            }
        }

        if is_valid {
            if let Some(value) = log.get(KEY) {
                moogsoft_metric.with_key(value.to_string_lossy());
            }
        }

        if is_valid {
            let mut source_set = false;
            let source_fields = vec![SOURCE, log_schema().host_key()];
            for source_field in source_fields {
                if let Some(value) = log.get(source_field) {
                    moogsoft_metric.with_source(value.to_string_lossy());
                    source_set = true;
                    break;
                }
            }

            if !source_set {
                if let Some(hostname) = get_hostname() {
                    moogsoft_metric.with_source(hostname);
                } else {
                    trace!("LogEvent cannot be converted as it is missing a source value.");
                    is_valid = false;
                }
            }
        }

        if is_valid {
            let mut time_set = false;
            let time_fields = vec![TIME, log_schema().timestamp_key()];
            for time_field in time_fields {
                if let Some(value) = log.get(time_field) {
                    match value {
                        Value::Integer(data) => {
                            moogsoft_metric.with_time(*data);
                            time_set = true;
                            break;
                        }
                        Value::Timestamp(data) => {
                            moogsoft_metric.with_time(data.timestamp());
                            time_set = true;
                            break;
                        }
                        _ => {
                            trace!("LogEvent cannot be converted because one of the time fields is invalid.");

                            // Not really set but since we are discarding we don't need to set it
                            time_set = true;

                            is_valid = false;
                        }
                    }
                }
            }

            if !time_set {
                moogsoft_metric.with_time(Utc::now().timestamp());
            }
        }

        let standard_fields = vec![
            METRIC,
            DATA,
            KEY,
            SOURCE,
            TIME,
            log_schema().host_key(),
            log_schema().timestamp_key(),
        ];
        for (field, value) in log.all_fields() {
            if !standard_fields.contains(&field.as_str()) {
                moogsoft_metric.tags.insert(field, value.to_string_lossy());
            }
        }

        if is_valid {
            Ok(moogsoft_metric)
        } else {
            Err(())
        }
    }
}

impl TryFrom<Metric> for MoogsoftMetric {
    type Error = ();

    fn try_from(metric: Metric) -> Result<Self, Self::Error> {
        match metric.data.value {
            Gauge { value } => {
                let mut moogsoft_metric: MoogsoftMetric = Default::default();
                let mut is_valid = true;

                if MetricKind::Incremental == metric.data.kind {
                    trace!("Only absolute gauge metrics can be sent.");
                    is_valid = false;
                }

                if is_valid {
                    moogsoft_metric.with_metric(metric.name().to_owned());
                    moogsoft_metric.with_data(value);

                    if let Some(key) = metric.namespace() {
                        moogsoft_metric.with_key(key.to_owned());
                    }

                    if let Some(timestamp) = metric.data.timestamp {
                        moogsoft_metric.with_time(timestamp.timestamp());
                    } else {
                        moogsoft_metric.with_time(Utc::now().timestamp());
                    }

                    if let Some(tags) = metric.tags() {
                        moogsoft_metric.with_tags(tags.clone());
                    }

                    if let Some(host) = metric.tag_value(log_schema().host_key()) {
                        moogsoft_metric.with_source(host);
                    } else if let Some(hostname) = get_hostname() {
                        moogsoft_metric.with_source(hostname);
                    } else {
                        trace!("Metric cannot be converted as it is missing a source value.");
                        is_valid = false;
                    }
                }

                if is_valid {
                    Ok(moogsoft_metric)
                } else {
                    Err(())
                }
            }
            _ => {
                trace!("Ignoring {} as it is not a gauge metric.", metric.name());
                Err(())
            }
        }
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "moogsoft")]
impl SinkConfig for MoogsoftSinkConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        // TODO: In the future we might want to expose some of the http configuration
        //       such as batching and retries in the Moogsoft config.
        //       For now certain default are ok

        let client = HttpClient::new(None)?;

        // For now no healthcheck needed on startup
        let healthcheck = future::ok(()).boxed();

        // Built (not from config)
        let request_config: RequestConfig = Default::default();

        // Build the tower request settings
        let request = request_config.tower.unwrap_with(&REQUEST_DEFAULTS);

        // Built (not from config)
        let batch_config: BatchConfig = Default::default();

        // Build the batch config (evolve as needed)
        let batch = BatchSettings::default()
            .bytes(bytesize::mib(10u64))
            .timeout(1)
            .parse_config(batch_config)?;

        let sink = BatchedHttpSink::new(
            self.clone(),
            Buffer::new(batch.size, Compression::None),
            request,
            batch.timeout,
            client,
            cx.acker(),
        )
        .sink_map_err(|error| error!(message = "Fatal Moogsoft sink error.", %error));

        let sink = super::VectorSink::Sink(Box::new(sink));

        Ok((sink, healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Any
    }

    fn sink_type(&self) -> &'static str {
        "moogsoft"
    }
}

#[async_trait::async_trait]
impl HttpSink for MoogsoftSinkConfig {
    type Input = Vec<u8>;
    type Output = Vec<u8>;

    fn encode_event(&self, event: Event) -> Option<EncodedEvent<Self::Input>> {
        match event {
            Event::Log(log) => encode_log_event(log),
            Event::Metric(metric) => encode_metric_event(metric),
        }
    }

    async fn build_request(&self, mut body: Self::Output) -> crate::Result<http::Request<Vec<u8>>> {
        let method = Method::POST;
        let uri: Uri = self.uri.uri.clone();

        body.insert(0, b'[');
        body.pop(); // remove trailing comma from last record
        body.push(b']');

        let ct = "application/json";

        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("Content-Type", ct)
            .header("apikey", self.api_key.clone());

        let request = builder.body(body).unwrap();

        Ok(request)
    }
}

fn encode_log_event(log: LogEvent) -> Option<EncodedEvent<Vec<u8>>> {
    if let Ok(metric) = MoogsoftMetric::try_from(log) {
        encode_moogsoft_metric(metric)
    } else {
        emit!(LogEventDiscarded);
        None
    }
}

fn encode_metric_event(metric: Metric) -> Option<EncodedEvent<Vec<u8>>> {
    if let Ok(metric) = MoogsoftMetric::try_from(metric) {
        encode_moogsoft_metric(metric)
    } else {
        emit!(MetricDiscarded);
        None
    }
}

fn encode_moogsoft_metric(metric: MoogsoftMetric) -> Option<EncodedEvent<Vec<u8>>> {
    let mut body = serde_json::to_vec(&metric)
        .map_err(|error| panic!("Unable to encode into JSON: {}", error))
        .ok()?;
    body.push(b',');

    emit!(MoogsoftMetricEncoded {
        byte_size: body.len(),
    });

    Some(EncodedEvent {
        item: body,
        metadata: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MetricValue;
    use crate::sinks::util::test::build_test_server;
    use crate::test_util::next_addr;
    use futures::{stream, StreamExt};
    use http::request::Parts;

    #[test]
    fn test_generate_config() {
        crate::test_util::test_generate_config::<MoogsoftSinkConfig>();
    }

    #[test]
    fn test_convert_log1_to_moogsoft_metric() {
        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now().timestamp();
        log_event.insert(METRIC, "cpu");
        log_event.insert(DATA, 10.5);
        log_event.insert(KEY, "system");
        log_event.insert(SOURCE, "some.machine");
        log_event.insert(TIME, timestamp_now);

        if let Ok(metric) = MoogsoftMetric::try_from(log_event) {
            assert_eq!(metric.metric, "cpu");
            assert_eq!(metric.data, 10.5);
            assert_eq!(metric.key, "system");
            assert_eq!(metric.source, "some.machine");
            assert_eq!(metric.time, timestamp_now);
            assert_eq!(metric.tags.len(), 0);
        } else {
            panic!("A Moogsoft metric should be created")
        }
    }

    #[test]
    fn test_convert_log2_to_moogsoft_metric() {
        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now();
        log_event.insert(METRIC, "cpu");
        log_event.insert(DATA, 11.5);
        log_event.insert(KEY, "system");
        log_event.insert(SOURCE, "some.machine");
        log_event.insert(TIME, timestamp_now);
        log_event.insert("application", "chrome");
        log_event.insert("duration", 10);

        if let Ok(metric) = MoogsoftMetric::try_from(log_event) {
            assert_eq!(metric.metric, "cpu");
            assert_eq!(metric.data, 11.5);
            assert_eq!(metric.key, "system");
            assert_eq!(metric.source, "some.machine");
            assert_eq!(metric.time, timestamp_now.timestamp());
            assert_eq!(metric.tags.len(), 2);
        } else {
            panic!("A Moogsoft metric should be created")
        }
    }

    #[test]
    fn test_convert_log3_to_moogsoft_metric() {
        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now();
        log_event.insert(METRIC, "cpu");
        log_event.insert(DATA, 12.5);
        log_event.insert(KEY, "system");
        log_event.insert(log_schema().host_key(), "some.machine");
        log_event.insert(log_schema().timestamp_key(), timestamp_now);

        if let Ok(metric) = MoogsoftMetric::try_from(log_event) {
            assert_eq!(metric.metric, "cpu");
            assert_eq!(metric.data, 12.5);
            assert_eq!(metric.key, "system");
            assert_eq!(metric.source, "some.machine");
            assert_eq!(metric.time, timestamp_now.timestamp());
        } else {
            panic!("A Moogsoft metric should be created")
        }
    }

    #[test]
    fn test_convert_log_missing_metric_to_moogsoft_metric() {
        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now().timestamp();
        log_event.insert(DATA, 10.5);
        log_event.insert(KEY, "system");
        log_event.insert(SOURCE, "some.machine");
        log_event.insert(TIME, timestamp_now);

        if MoogsoftMetric::try_from(log_event).is_ok() {
            panic!("A Moogsoft metric should not be created")
        }
    }

    #[test]
    fn test_convert_log_missing_data_to_moogsoft_metric() {
        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now().timestamp();
        log_event.insert(METRIC, "cpu");
        log_event.insert(KEY, "system");
        log_event.insert(SOURCE, "some.machine");
        log_event.insert(TIME, timestamp_now);

        if MoogsoftMetric::try_from(log_event).is_ok() {
            panic!("A Moogsoft metric should not be created")
        }
    }

    #[test]
    fn test_convert_log_bad_data_to_moogsoft_metric() {
        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now().timestamp();
        log_event.insert(METRIC, "cpu");
        log_event.insert(DATA, "Not a number");
        log_event.insert(KEY, "system");
        log_event.insert(SOURCE, "some.machine");
        log_event.insert(TIME, timestamp_now);

        if MoogsoftMetric::try_from(log_event).is_ok() {
            panic!("A Moogsoft metric should not be created")
        }
    }

    #[test]
    fn test_convert_absolute_metric_to_moogsoft_metric() {
        let value = 90.5;
        let timestamp_now = Utc::now();
        let mut tags = MetricTags::new();
        tags.insert(
            log_schema().host_key().to_owned(),
            "some.machine".to_owned(),
        );
        let metric_event = Metric::new("cpu", MetricKind::Absolute, MetricValue::Gauge { value })
            .with_namespace(Some("system"))
            .with_tags(Some(tags))
            .with_timestamp(Some(timestamp_now));

        if let Ok(metric) = MoogsoftMetric::try_from(metric_event) {
            assert_eq!(metric.metric, "cpu");
            assert_eq!(metric.data, 90.5);
            assert_eq!(metric.key, "system");
            assert_eq!(metric.source, "some.machine");
            assert_eq!(metric.time, timestamp_now.timestamp());
            assert_eq!(metric.tags.len(), 1);
        } else {
            panic!("A Moogsoft metric should be created")
        }
    }

    #[test]
    fn test_convert_incremental_metric_to_moogsoft_metric() {
        let value = 90.5;
        let timestamp_now = Utc::now();
        let mut tags = MetricTags::new();
        tags.insert(
            log_schema().host_key().to_owned(),
            "some.machine".to_owned(),
        );
        let metric_event =
            Metric::new("cpu", MetricKind::Incremental, MetricValue::Gauge { value })
                .with_namespace(Some("system"))
                .with_tags(Some(tags))
                .with_timestamp(Some(timestamp_now));

        if MoogsoftMetric::try_from(metric_event).is_ok() {
            panic!("A Moogsoft metric should not be created from an incremental")
        }
    }

    #[tokio::test]
    async fn test_end_to_end_log_event() {
        let in_addr = next_addr();
        let (rx, trigger, server) = build_test_server(in_addr);
        tokio::spawn(server);

        let config = MoogsoftSinkConfig {
            api_key: "secret".to_string(),
            uri: uri_for_authority(in_addr.to_string().as_str()),
        };

        let cx = SinkContext::new_test();

        let (sink, _) = config.build(cx).await.unwrap();

        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now().timestamp();
        log_event.insert(METRIC, "cpu");
        log_event.insert(DATA, 10.5);
        log_event.insert(KEY, "system");
        log_event.insert(SOURCE, "some.machine");
        log_event.insert(TIME, timestamp_now);

        let event = Event::Log(log_event);

        let events = vec![event];
        sink.run(stream::iter(events)).await.unwrap();

        drop(trigger);

        rx.map(|(parts, body)| {
            check_headers(parts);

            let body = String::from_utf8(body.to_vec()).unwrap();
            let mut metrics: Vec<MoogsoftMetric> = serde_json::from_str(body.as_str()).unwrap();
            let metric = metrics.pop().unwrap();
            assert_eq!(metric.metric, "cpu");
            assert_eq!(metric.data, 10.5);
            assert_eq!(metric.key, "system");
            assert_eq!(metric.source, "some.machine");
            assert_eq!(metric.time, timestamp_now);
            assert_eq!(metric.tags.len(), 0);
        })
        .collect::<Vec<_>>()
        .await;
    }

    #[tokio::test]
    async fn test_end_to_end_metric_event() {
        let in_addr = next_addr();
        let (rx, trigger, server) = build_test_server(in_addr);
        tokio::spawn(server);

        let config = MoogsoftSinkConfig {
            api_key: "secret".to_string(),
            uri: uri_for_authority(in_addr.to_string().as_str()),
        };

        let cx = SinkContext::new_test();

        let (sink, _) = config.build(cx).await.unwrap();

        let value = 90.5;
        let timestamp_now = Utc::now();
        let mut tags = MetricTags::new();
        tags.insert(
            log_schema().host_key().to_owned(),
            "some.machine".to_owned(),
        );
        let metric_event = Metric::new("cpu", MetricKind::Absolute, MetricValue::Gauge { value })
            .with_namespace(Some("system"))
            .with_tags(Some(tags))
            .with_timestamp(Some(timestamp_now));

        let event = Event::Metric(metric_event);

        let events = vec![event];
        sink.run(stream::iter(events)).await.unwrap();

        drop(trigger);

        rx.map(|(parts, body)| {
            check_headers(parts);

            let body = String::from_utf8(body.to_vec()).unwrap();
            let mut metrics: Vec<MoogsoftMetric> = serde_json::from_str(body.as_str()).unwrap();
            let metric = metrics.pop().unwrap();
            assert_eq!(metric.metric, "cpu");
            assert_eq!(metric.data, 90.5);
            assert_eq!(metric.key, "system");
            assert_eq!(metric.source, "some.machine");
            assert_eq!(metric.time, timestamp_now.timestamp());
            assert_eq!(metric.tags.len(), 1);
        })
        .collect::<Vec<_>>()
        .await;
    }

    fn check_headers(parts: Parts) {
        assert_eq!(parts.method, "POST");
        assert_eq!(parts.uri.path(), "/v1/integrations/metrics");
        let headers = parts.headers;
        assert_eq!(headers["Content-Type"], "application/json");
        assert_eq!(headers["apikey"], "secret");
    }
}
