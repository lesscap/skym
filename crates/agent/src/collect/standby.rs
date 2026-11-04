//! Blue/green standbys: a stopped service whose other project runs it.

use super::Workload;
use crate::config::BlueGreen;
use skym_core::model::RunState;
use skym_core::subject::WorkloadKey;

/// Marks `stop_allowed` on stopped workloads of a pair whose other project runs the same
/// service. Only from a complete listing: a container that was not inspected may be the one
/// running.
pub fn mark(pairs: &[BlueGreen], complete: bool, workloads: Vec<Workload>) -> Vec<Workload> {
    if !complete || pairs.is_empty() {
        return workloads;
    }
    let running =
        |k: &WorkloadKey| workloads.iter().any(|w| w.key == *k && w.state.run == RunState::Running);
    let standby = |w: &Workload| {
        w.state.run != RunState::Running
            && pairs.iter().any(|pair| {
                pair.services.contains(&w.key.service)
                    && pair.projects.contains(&w.key.project)
                    && pair
                        .projects
                        .iter()
                        .filter(|p| **p != w.key.project)
                        .any(|p| running(&WorkloadKey { project: p.clone(), ..w.key.clone() }))
            })
    };
    let marked: Vec<bool> = workloads.iter().map(standby).collect();
    workloads
        .into_iter()
        .zip(marked)
        .map(|(mut w, stop_allowed)| {
            w.state.stop_allowed = stop_allowed;
            w
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workload(project: &str, service: &str, run: RunState) -> Workload {
        let mut w = skym_core::fixtures::full_report().workloads[0].clone();
        w.key =
            WorkloadKey { host: "yi1".into(), project: project.into(), service: service.into() };
        w.state.run = run;
        Workload { key: w.key, facts: w.facts.unwrap(), state: w.state }
    }

    fn allowed(complete: bool, workloads: Vec<Workload>) -> Vec<String> {
        let pair = BlueGreen {
            projects: vec!["prod".into(), "prod-green".into()],
            services: vec!["app".into(), "edge".into()],
        };
        mark(&[pair], complete, workloads)
            .into_iter()
            .filter(|w| w.state.stop_allowed)
            .map(|w| format!("{}/{}", w.key.project, w.key.service))
            .collect()
    }

    #[test]
    fn a_stopped_slot_is_a_standby_while_the_other_runs_the_service() {
        use RunState::{Exited, Running};
        let idle = || {
            vec![
                workload("prod", "app", Exited),
                workload("prod", "edge", Exited),
                workload("prod", "tasks", Exited),
                workload("prod-green", "app", Running),
                workload("prod-green", "edge", Running),
                workload("other", "app", Exited),
            ]
        };
        assert_eq!(
            allowed(true, idle()),
            ["prod/app", "prod/edge"],
            "tasks and others are not paired"
        );
        assert!(allowed(false, idle()).is_empty(), "an incomplete listing proves nothing");
        let both_down =
            vec![workload("prod", "app", Exited), workload("prod-green", "app", Exited)];
        assert!(allowed(true, both_down).is_empty(), "nobody serves: both are down");
        let deploying =
            vec![workload("prod", "app", Running), workload("prod-green", "app", Running)];
        assert!(allowed(true, deploying).is_empty());
    }
}
