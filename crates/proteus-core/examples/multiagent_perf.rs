//! Actual AgentControl peers; inputs and Linux measurements live with the benchmark.
#[path = "../../../examples/performance/multi-agent/driver.rs"]
mod driver;
#[path = "../tests/support/model.rs"]
mod test_model;

fn main() -> anyhow::Result<()> {
    driver::run()
}
