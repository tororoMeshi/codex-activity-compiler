//! Codex の OTLP Trace を、Raw telemetry を保存せず活動記録へコンパイルする。

pub mod http;
pub mod normalize;
pub mod store;

#[cfg(test)]
mod tests;
