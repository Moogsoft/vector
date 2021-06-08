use crate::event::{LogEvent, Metric, Value};
use crate::{
    config::{log_schema, DataType, SinkConfig, SinkContext, SinkDescription},
    event::Event,
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
use std::str::FromStr;

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MoogsoftInternalLogsSinkConfig {
    pub api_key: String,
    pub uri: UriSerde,
}

impl Default for MoogsoftInternalLogsSinkConfig {
    fn default() -> Self {
        MoogsoftInternalLogsSinkConfig {
            api_key: default_api_key(),
            uri: default_uri(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
struct MoogsoftCollectorLog {
    level: LogLevel,
    message: String,
    component: Option<String>,
    timestamp: i64,
}

impl Default for MoogsoftCollectorLog {
    fn default() -> Self {
        MoogsoftCollectorLog {
            level: LogLevel::Error,
            message: String::new(),
            component: None,
            timestamp: Utc::now().timestamp(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl FromStr for LogLevel {
    type Err = ();

    fn from_str(input: &str) -> Result<LogLevel, Self::Err> {
        match input.to_ascii_lowercase().as_str() {
            "trace" => Ok(LogLevel::Trace),
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            _ => Err(()),
        }
    }
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
    SinkDescription::new::<MoogsoftInternalLogsSinkConfig>("moogsoft_internal_logs")
}

impl_generate_config_from_default!(MoogsoftInternalLogsSinkConfig);

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
        .path_and_query("/v2/collectors/uuid/logs")
        .build()
        .unwrap();
    UriSerde::from(uri)
}

const METADATA_KIND: &str = "metadata.kind";
const METADATA_LEVEL: &str = "metadata.level";
const EVENT: &str = "event";
const NAME: &str = "name";

impl MoogsoftCollectorLog {
    fn with_level(&mut self, level: LogLevel) -> &Self {
        self.level = level;
        self
    }

    fn with_message(&mut self, message: String) -> &Self {
        self.message = message;
        self
    }

    fn with_component(&mut self, component: Option<String>) -> &Self {
        self.component = component;
        self
    }

    fn with_timestamp(&mut self, timestamp: i64) -> &Self {
        self.timestamp = timestamp;
        self
    }
}

impl TryFrom<LogEvent> for MoogsoftCollectorLog {
    type Error = ();

    fn try_from(log: LogEvent) -> Result<Self, Self::Error> {
        let mut moogsoft_log: MoogsoftCollectorLog = Default::default();
        let mut is_valid = true;

        if let Some(kind) = log.get(METADATA_KIND) {
            // To be a valid log the kind must be event
            is_valid = kind.to_string_lossy().eq_ignore_ascii_case(EVENT);
        }

        if is_valid {
            if let Some(level) = log.get(METADATA_LEVEL) {
                if let Ok(level) = LogLevel::from_str(&*level.to_string_lossy()) {
                    moogsoft_log.with_level(level);
                } else {
                    trace!("LogEvent cannot be converted as it has an invalid log level.");
                    is_valid = false;
                }
            } else {
                trace!("LogEvent cannot be converted as it has no log level.");
                is_valid = false;
            }
        }

        if is_valid {
            if let Some(message) = log.get(log_schema().message_key()) {
                moogsoft_log.with_message(message.to_string_lossy());
            } else {
                trace!("LogEvent cannot be converted as it has no log message.");
                is_valid = false;
            }
        }

        if is_valid {
            if let Some(name) = log.get(NAME) {
                moogsoft_log.with_component(Some(name.to_string_lossy()));
            } else {
                moogsoft_log.with_component(None);
            }
        }

        if is_valid {
            if let Some(timestamp) = log.get(log_schema().timestamp_key()) {
                match timestamp {
                    Value::Integer(data) => {
                        moogsoft_log.with_timestamp(*data);
                    }
                    Value::Timestamp(data) => {
                        moogsoft_log.with_timestamp(data.timestamp());
                    }
                    _ => {
                        trace!("LogEvent cannot be converted the timestamp field is invalid.");
                        is_valid = false;
                    }
                }
            }
        }

        if is_valid {
            Ok(moogsoft_log)
        } else {
            Err(())
        }
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "moogsoft_internal_logs")]
impl SinkConfig for MoogsoftInternalLogsSinkConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        // TODO: In the future we might want to expose some of the http configuration
        //       such as batching and retries in the Moogsoft config.
        //       For now certain default are ok

        // TODO: At present this component does not generate any internal metrics
        //       although it probably should.

        // TODO: Create cue document describing this sink

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
        .sink_map_err(|error| error!(message = "Fatal Moogsoft internal logs sink error.", %error));

        let sink = super::VectorSink::Sink(Box::new(sink));

        Ok((sink, healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "moogsoft_internal_logs"
    }
}

#[async_trait::async_trait]
impl HttpSink for MoogsoftInternalLogsSinkConfig {
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
    if let Ok(log) = MoogsoftCollectorLog::try_from(log) {
        encode_moogsoft_collector_log(log)
    } else {
        None
    }
}

fn encode_metric_event(_: Metric) -> Option<EncodedEvent<Vec<u8>>> {
    warn!("Metric events should not be received by this sink.");
    None
}

fn encode_moogsoft_collector_log(log: MoogsoftCollectorLog) -> Option<EncodedEvent<Vec<u8>>> {
    let mut body = serde_json::to_vec(&log)
        .map_err(|error| panic!("Unable to encode into JSON: {}", error))
        .ok()?;
    body.push(b',');

    Some(EncodedEvent {
        item: body,
        metadata: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sinks::util::test::build_test_server;
    use crate::test_util::next_addr;
    use futures::{stream, StreamExt};
    use http::request::Parts;

    #[test]
    fn test_generate_config() {
        crate::test_util::test_generate_config::<MoogsoftInternalLogsSinkConfig>();
    }

    #[test]
    fn test_convert_log1_to_moogsoft_collector_log() {
        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now().timestamp();
        log_event.insert(METADATA_KIND, "event");
        log_event.insert(METADATA_LEVEL, "info");
        log_event.insert(log_schema().message_key(), "Hello World!");
        log_event.insert(log_schema().timestamp_key(), timestamp_now);

        if let Ok(log) = MoogsoftCollectorLog::try_from(log_event) {
            assert_eq!(log.timestamp, timestamp_now);
            assert_eq!(log.message, "Hello World!");
            assert_eq!(log.component, None);
            assert_eq!(log.level, LogLevel::Info);
        } else {
            panic!("A Moogsoft collector log should be created")
        }
    }

    #[test]
    fn test_convert_incorrect_kind_log_to_moogsoft_collector_log() {
        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now().timestamp();
        log_event.insert(METADATA_KIND, "Not Event");
        log_event.insert(METADATA_LEVEL, "info");
        log_event.insert(log_schema().message_key(), "Hello World!");
        log_event.insert(log_schema().timestamp_key(), timestamp_now);

        if MoogsoftCollectorLog::try_from(log_event).is_ok() {
            panic!("A Moogsoft collector log should not be created")
        }
    }

    #[tokio::test]
    async fn test_end_to_end_log_event() {
        let in_addr = next_addr();
        let (rx, trigger, server) = build_test_server(in_addr);
        tokio::spawn(server);

        let config = MoogsoftInternalLogsSinkConfig {
            api_key: "secret".to_string(),
            uri: uri_for_authority(in_addr.to_string().as_str()),
        };

        let cx = SinkContext::new_test();

        let (sink, _) = config.build(cx).await.unwrap();

        let mut log_event = LogEvent::default();
        let timestamp_now = Utc::now().timestamp();
        log_event.insert(METADATA_KIND, "event");
        log_event.insert(METADATA_LEVEL, "info");
        log_event.insert(log_schema().message_key(), "Hello World!");
        log_event.insert(log_schema().timestamp_key(), timestamp_now);

        let event = Event::Log(log_event);

        let events = vec![event];
        sink.run(stream::iter(events)).await.unwrap();

        drop(trigger);

        rx.map(|(parts, body)| {
            check_headers(parts);

            let body = String::from_utf8(body.to_vec()).unwrap();
            let mut logs: Vec<MoogsoftCollectorLog> = serde_json::from_str(body.as_str()).unwrap();
            let log = logs.pop().unwrap();
            assert_eq!(log.timestamp, timestamp_now);
            assert_eq!(log.message, "Hello World!");
            assert_eq!(log.component, None);
            assert_eq!(log.level, LogLevel::Info);
        })
        .collect::<Vec<_>>()
        .await;
    }

    fn check_headers(parts: Parts) {
        assert_eq!(parts.method, "POST");
        assert_eq!(parts.uri.path(), "/v2/collectors/uuid/logs");
        let headers = parts.headers;
        assert_eq!(headers["Content-Type"], "application/json");
        assert_eq!(headers["apikey"], "secret");
    }
}
