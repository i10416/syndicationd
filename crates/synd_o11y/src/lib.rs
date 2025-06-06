//! syndicationd observability crate providing features
//! related to tracing, opentelemetry, and other observability
//! functionalities.
#![warn(rustdoc::broken_intra_doc_links)]

use ::opentelemetry::KeyValue;

pub mod health_check;
pub mod opentelemetry;
pub mod tracing_subscriber;

pub use opentelemetry::OpenTelemetryGuard;
pub use tracing_subscriber::opentelemetry_layer;

/// Request id key for opentelemetry baggage
pub const REQUEST_ID_KEY: &str = "request.id";

/// Generate random request id
pub fn request_id() -> String {
    // https://stackoverflow.com/questions/54275459/how-do-i-create-a-random-string-by-sampling-from-alphanumeric-characters
    use rand::distr::{Alphanumeric, SampleString};
    Alphanumeric.sample_string(&mut rand::rng(), 10)
}

/// Generate random request id key value
pub fn request_id_key_value() -> KeyValue {
    KeyValue::new(REQUEST_ID_KEY, request_id())
}
