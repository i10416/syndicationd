use std::time::Duration;

use opentelemetry::global;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    metrics::{Instrument, PeriodicReader, SdkMeterProvider, Stream, View},
};
use tracing::{Metadata, Subscriber};
use tracing_opentelemetry::MetricsLayer;
use tracing_subscriber::{Layer, filter::filter_fn, layer::Filter, registry::LookupSpan};

pub mod macros;

pub const METRICS_EVENT_TARGET: &str = "metrics";

pub fn metrics_event_filter<S: Subscriber>() -> impl Filter<S> {
    filter_fn(|metadata: &Metadata<'_>| metadata.target() != METRICS_EVENT_TARGET)
}

pub fn layer<S>(
    endpoint: impl Into<String>,
    resource: Resource,
    interval: Duration,
) -> (impl Layer<S>, SdkMeterProvider)
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    let provider = init_meter_provider(endpoint, resource, interval);
    (MetricsLayer::new(provider.clone()), provider)
}

fn init_meter_provider(
    endpoint: impl Into<String>,
    resource: Resource,
    interval: Duration,
) -> SdkMeterProvider {
    // Currently OtelpMetricPipeline does not provide a way to set up views.
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .unwrap();

    let reader = PeriodicReader::builder(exporter)
        .with_interval(interval)
        .build();

    let view = view();
    let meter_provider_builder = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_reader(reader)
        .with_view(view);

    #[cfg(feature = "opentelemetry-stdout")]
    let stdout_reader = {
        let exporter = opentelemetry_stdout::MetricExporterBuilder::default().build();
        PeriodicReader::builder(exporter)
            .with_interval(Duration::from_secs(60))
            .build()
    };
    #[cfg(feature = "opentelemetry-stdout")]
    let meter_provider_builder = meter_provider_builder.with_reader(stdout_reader);

    let meter_provider = meter_provider_builder.build();

    global::set_meter_provider(meter_provider.clone());

    meter_provider
}

fn view() -> impl View {
    |instrument: &Instrument| -> Option<Stream> {
        tracing::debug!("{instrument:?}");

        match instrument.name.as_ref() {
            "graphql.duration" => Some(
                Stream::new()
                    .name(instrument.name.clone())
                    .description("graphql response duration")
                    // Currently we could not ingest metrics with Arregation::Base2ExponentialHistogram in grafanacloud
                    .aggregation(
                        opentelemetry_sdk::metrics::Aggregation::ExplicitBucketHistogram {
                            // https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#http-server
                            boundaries: vec![
                                0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5,
                                5.0, 7.5, 10.0,
                            ],
                            record_min_max: false,
                        },
                    )
                    // https://opentelemetry.io/docs/specs/semconv/general/metrics/#instrument-units
                    .unit("s"),
            ),
            name => {
                tracing::debug!(name, "There is no explicit view");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use std::{net::SocketAddr, time::Duration};

    use opentelemetry::KeyValue;
    use opentelemetry_proto::tonic::{
        collector::metrics::v1::{
            ExportMetricsServiceRequest, ExportMetricsServiceResponse,
            metrics_service_server::{MetricsService, MetricsServiceServer},
        },
        metrics::v1::{AggregationTemporality, metric::Data, number_data_point::Value},
    };
    use tokio::sync::mpsc::UnboundedSender;
    use tonic::transport::Server;
    use tracing::dispatcher;
    use tracing_subscriber::{Registry, layer::SubscriberExt};

    use super::*;

    type Request = tonic::Request<ExportMetricsServiceRequest>;

    struct DumpMetrics {
        tx: UnboundedSender<Request>,
    }

    #[tonic::async_trait]
    impl MetricsService for DumpMetrics {
        async fn export(
            &self,
            request: tonic::Request<ExportMetricsServiceRequest>,
        ) -> Result<tonic::Response<ExportMetricsServiceResponse>, tonic::Status> {
            self.tx.send(request).unwrap();

            Ok(tonic::Response::new(ExportMetricsServiceResponse {
                partial_success: None, // means success
            }))
        }
    }

    fn f1() {
        tracing::info!(monotonic_counter.f1 = 1, key1 = "val1");
    }

    fn f2() {
        tracing::info!(histogram.graphql.duration = 0.5);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn layer_test() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let dump = MetricsServiceServer::new(DumpMetrics { tx });
        let addr: SocketAddr = ([127, 0, 0, 1], 48101).into();
        let server = Server::builder().add_service(dump).serve(addr);
        let _server = tokio::task::spawn(server);
        let resource = resource();
        // The default interval is 60 seconds, which slows down the test
        let interval = Duration::from_millis(100);
        let (layer, _provider) = layer("https://localhost:48101", resource.clone(), interval);
        let subscriber = Registry::default().with(layer);
        let dispatcher = tracing::Dispatch::new(subscriber);

        dispatcher::with_default(&dispatcher, || {
            f1();
        });

        let req = rx.recv().await.unwrap().into_inner();
        assert_eq!(req.resource_metrics.len(), 1);

        let metric1 = req.resource_metrics[0].clone();
        insta::with_settings!({
            description => " metric 1 resource",
        }, {
            insta::assert_yaml_snapshot!("layer_test_metric_1_resource", metric1.resource, {
                ".attributes" => insta::sorted_redaction(),
            });
        });

        let metric1 = metric1.scope_metrics[0].metrics[0].clone();
        assert_eq!(&metric1.name, "f1");

        let Some(Data::Sum(sum)) = metric1.data else {
            panic!("metric1 data is not sum")
        };
        assert!(sum.is_monotonic);
        assert_eq!(
            sum.aggregation_temporality,
            AggregationTemporality::Cumulative as i32
        );

        let data = sum.data_points[0].clone();
        assert_eq!(data.value, Some(Value::AsInt(1)));
        insta::with_settings!({
            description => " metric 1 datapoint attributes",
        }, {
            insta::assert_yaml_snapshot!("layer_test_metric_1_datapoint_attributes", data.attributes);
        });

        dispatcher::with_default(&dispatcher, || {
            f2();
        });
        let req = rx.recv().await.unwrap().into_inner();
        insta::with_settings!({
            description => "graphql duration histogram metrics",
        }, {
            insta::assert_yaml_snapshot!("layer_test_metrics_graphql_histogram", req, {
                ".**.startTimeUnixNano" => "[UNIX_TIMESTAMP]",
                ".**.timeUnixNano" => "[UNIX_TIMESTAMP]",
                ".**.scope.version" => "[INSTRUMENT_LIB_VERSION]",
                ".**.attributes" => insta::sorted_redaction(),
            });
        });
    }

    fn resource() -> Resource {
        Resource::builder()
            .with_attributes([KeyValue::new("service.name", "test")])
            .build()
    }
}
