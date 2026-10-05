//! Helpers shared by the server's integration tests.

use skym_core::model::RunState;
use skym_core::report::Report;
use skym_server::config::AppConfig;

/// Workload `i` of the report exited with an error.
pub fn down(mut r: Report, i: usize) -> Report {
    r.workloads[i].state.run = RunState::Exited;
    r.workloads[i].state.exit_code = Some(1);
    r
}

/// A listed application with nothing but its id and environment.
pub fn app_config(id: &str, env: Option<&str>) -> AppConfig {
    AppConfig {
        id: id.parse().unwrap(),
        name: None,
        env: env.map(String::from),
        note: None,
        tags: vec![],
        probes: vec![],
    }
}
