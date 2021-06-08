use crate::event::{LogEvent, Metric, MetricValue};
use crate::{
    config::{DataType, SinkConfig, SinkContext, SinkDescription},
    event::Event,
    get_version,
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

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MoogsoftControllerSinkConfig {
    pub api_key: String,
    pub uri: UriSerde,
}

impl Default for MoogsoftControllerSinkConfig {
    fn default() -> Self {
        MoogsoftControllerSinkConfig {
            api_key: default_api_key(),
            uri: default_uri(),
        }
    }
}

// TODO: Add a few more useful attributes e.g.
//       architecture, platform, current-cpu, current-diskspace
//       Could possibly look at using the sysinfo crate.
#[derive(Deserialize, Serialize, Debug)]
struct MoogsoftCollectorHeartbeat {
    uptime: f64,
    version: String,
    host: Option<String>,
    timestamp: i64,
}

impl Default for MoogsoftCollectorHeartbeat {
    fn default() -> Self {
        MoogsoftCollectorHeartbeat {
            uptime: 0_f64,
            version: get_version(),
            host: get_hostname(),
            timestamp: Utc::now().timestamp(),
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
    SinkDescription::new::<MoogsoftControllerSinkConfig>("moogsoft_controller")
}

impl_generate_config_from_default!(MoogsoftControllerSinkConfig);

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
        .path_and_query("/v2/collectors/uuid/heartbeats")
        .build()
        .unwrap();
    UriSerde::from(uri)
}

fn get_hostname() -> Option<String> {
    crate::get_hostname().ok()
}

const UPTIME_SECONDS: &str = "uptime_seconds";

impl MoogsoftCollectorHeartbeat {
    fn with_uptime(&mut self, uptime: f64) -> &Self {
        self.uptime = uptime;
        self
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "moogsoft_controller")]
impl SinkConfig for MoogsoftControllerSinkConfig {
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
        .sink_map_err(|error| error!(message = "Fatal Moogsoft controller sink error.", %error));

        let sink = super::VectorSink::Sink(Box::new(sink));

        Ok((sink, healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "moogsoft_controller"
    }
}

#[async_trait::async_trait]
impl HttpSink for MoogsoftControllerSinkConfig {
    type Input = Vec<u8>;
    type Output = Vec<u8>;

    fn encode_event(&self, event: Event) -> Option<EncodedEvent<Self::Input>> {
        match event {
            Event::Log(log) => handle_log_event(log),
            Event::Metric(metric) => handle_metric_event(metric),
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

fn handle_log_event(_: LogEvent) -> Option<EncodedEvent<Vec<u8>>> {
    warn!("Metric events should not be received by this sink.");
    None
}

// TODO: Handle asynchronous messages (possibly in response to heartbeat
//  but it could also be another endpoint)
fn handle_metric_event(metric: Metric) -> Option<EncodedEvent<Vec<u8>>> {
    let name = metric.name();
    if name.eq_ignore_ascii_case(UPTIME_SECONDS) {
        // TODO: Perhaps only send every x number to reduce the traffic
        let mut heartbeat = MoogsoftCollectorHeartbeat::default();

        match metric.data.value {
            MetricValue::Gauge { value } => {
                heartbeat.with_uptime(value);
            }
            _ => {
                warn!("Unable to obtain uptime, leaving it as default.");
            }
        }

        encode_heartbeat(heartbeat)
    } else {
        // The Moogsoft controller sink ignores all internal metrics apart from uptime
        None
    }
}

fn encode_heartbeat(heartbeat: MoogsoftCollectorHeartbeat) -> Option<EncodedEvent<Vec<u8>>> {
    let mut body = serde_json::to_vec(&heartbeat)
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
    use crate::event::MetricKind;
    use crate::sinks::util::test::build_test_server;
    use crate::test_util::next_addr;
    use futures::{stream, StreamExt};
    use http::request::Parts;

    #[test]
    fn test_generate_config() {
        crate::test_util::test_generate_config::<MoogsoftControllerSinkConfig>();
    }

    #[tokio::test]
    async fn test_end_to_end_uptime_event() {
        let in_addr = next_addr();
        let (rx, trigger, server) = build_test_server(in_addr);
        tokio::spawn(server);

        let config = MoogsoftControllerSinkConfig {
            api_key: "secret".to_string(),
            uri: uri_for_authority(in_addr.to_string().as_str()),
        };

        let cx = SinkContext::new_test();

        let (sink, _) = config.build(cx).await.unwrap();

        let value = 90.0;
        let timestamp_now = Utc::now();
        let metric_event = Metric::new(
            UPTIME_SECONDS,
            MetricKind::Absolute,
            MetricValue::Gauge { value },
        )
        .with_namespace(Some("system"))
        .with_timestamp(Some(timestamp_now));

        let event = Event::Metric(metric_event);

        let events = vec![event];
        sink.run(stream::iter(events)).await.unwrap();

        drop(trigger);

        rx.map(|(parts, body)| {
            check_headers(parts);

            let body = String::from_utf8(body.to_vec()).unwrap();
            let mut heartbeats: Vec<MoogsoftCollectorHeartbeat> =
                serde_json::from_str(body.as_str()).unwrap();
            let heartbeat = heartbeats.pop().unwrap();
            assert_eq!(heartbeat.timestamp, timestamp_now.timestamp());
            assert_eq!(heartbeat.host, get_hostname());
            assert_eq!(heartbeat.version, get_version());
            assert_eq!(heartbeat.uptime, 90.0);
        })
        .collect::<Vec<_>>()
        .await;
    }

    fn check_headers(parts: Parts) {
        assert_eq!(parts.method, "POST");
        assert_eq!(parts.uri.path(), "/v2/collectors/uuid/heartbeat");
        let headers = parts.headers;
        assert_eq!(headers["Content-Type"], "application/json");
        assert_eq!(headers["apikey"], "secret");
    }
}
