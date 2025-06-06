use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    logs::{BatchConfig, SdkLoggerProvider},
};
use tracing::Subscriber;
use tracing_subscriber::{Layer, registry::LookupSpan};

pub fn layer<S>(
    endpoint: impl Into<String>,
    resource: Resource,
    // TODO: how to use BatchConfig after 0.27 ?
    _batch_config: BatchConfig,
) -> (impl Layer<S>, SdkLoggerProvider)
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .unwrap();
    let provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();

    (OpenTelemetryTracingBridge::new(&provider), provider)
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use opentelemetry::KeyValue;
    use opentelemetry_proto::tonic::collector::logs::v1::{
        ExportLogsServiceRequest, ExportLogsServiceResponse,
        logs_service_server::{LogsService, LogsServiceServer},
    };
    use opentelemetry_sdk::logs::BatchConfigBuilder;
    use tokio::sync::mpsc::UnboundedSender;
    use tonic::transport::Server;
    use tracing::dispatcher;
    use tracing_subscriber::{Registry, layer::SubscriberExt};

    use super::*;

    type Request = tonic::Request<ExportLogsServiceRequest>;

    struct Dumplogs {
        tx: UnboundedSender<Request>,
    }

    #[tonic::async_trait]
    impl LogsService for Dumplogs {
        async fn export(
            &self,
            request: tonic::Request<ExportLogsServiceRequest>,
        ) -> Result<tonic::Response<ExportLogsServiceResponse>, tonic::Status> {
            self.tx.send(request).unwrap();

            Ok(tonic::Response::new(ExportLogsServiceResponse {
                partial_success: None, // means success
            }))
        }
    }

    fn f1() {
        tracing::info!("f1_message");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn layer_test() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let dump = LogsServiceServer::new(Dumplogs { tx });
        let addr: SocketAddr = ([127, 0, 0, 1], 48102).into();
        let server = Server::builder().add_service(dump).serve(addr);
        let _server = tokio::task::spawn(server);
        let resource = resource();
        // The default interval is 1 seconds, which slows down the test
        let config = BatchConfigBuilder::default()
            .with_scheduled_delay(Duration::from_millis(100))
            .build();
        let (layer, provider) = layer("https://localhost:48102", resource.clone(), config);
        let subscriber = Registry::default().with(layer);
        let dispatcher = tracing::Dispatch::new(subscriber);

        dispatcher::with_default(&dispatcher, || {
            f1();
        });
        provider.shutdown().unwrap();

        let req = rx.recv().await.unwrap().into_inner();
        assert_eq!(req.resource_logs.len(), 1);

        let log1 = req.resource_logs[0].clone();
        insta::with_settings!({
            description => " log 1 resource",
        }, {
            insta::assert_yaml_snapshot!("layer_test_log_1_resource", log1.resource,{
                ".attributes"  => insta::sorted_redaction(),
            });
        });

        let record = log1.scope_logs[0].log_records[0].clone();
        insta::with_settings!({
            description => " log 1 record",
        }, {
            insta::assert_yaml_snapshot!("layer_test_log_1_record", record, {
                ".observedTimeUnixNano" => "[OBSERVED_TIME_UNIX_NANO]",
            });
        });
    }

    fn resource() -> Resource {
        Resource::builder()
            .with_attributes([KeyValue::new("service.name", "test")])
            .build()
    }
}
