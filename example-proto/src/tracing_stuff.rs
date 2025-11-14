// Construct TracerProvider for OpenTelemetryLayer
fn get_tracer_provider() -> opentelemetry_sdk::trace::SdkTracerProvider {
    let resource = opentelemetry_sdk::Resource::builder()
            .with_service_name(env!("CARGO_PKG_NAME"))
            .with_schema_url(
                [
                    opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                    opentelemetry::KeyValue::new("service.environment", "laptop"),
                ],
                "come on dude",
            )
            .build();


    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()
        .unwrap();

    opentelemetry_sdk::trace::SdkTracerProvider::builder()
        // Customize sampling strategy
        .with_sampler(opentelemetry_sdk::trace::Sampler::ParentBased(Box::new(opentelemetry_sdk::trace::Sampler::TraceIdRatioBased(
            1.0,
        ))))
        // If export trace to AWS X-Ray, you can use XrayIdGenerator
        .with_id_generator(opentelemetry_sdk::trace::RandomIdGenerator::default())
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build()
}

pub fn set_default_tracer(name: &'static str) {
    let tracer = opentelemetry::trace::TracerProvider::tracer(&get_tracer_provider(), name);
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = tracing_subscriber::layer::SubscriberExt::with(tracing_subscriber::Registry::default(), telemetry);
    tracing::subscriber::set_global_default(subscriber).expect("can set tracer provider");
}
