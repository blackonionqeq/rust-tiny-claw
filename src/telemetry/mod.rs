#[derive(Debug, Default)]
pub struct Telemetry;

impl Telemetry {
    pub fn name(&self) -> &'static str {
        "telemetry"
    }
}
