use crate::collect::Collected;
use jiff::Timestamp;
use skym_core::model::HostState;
use skym_core::report::{Report, WorkloadReport, facts_hash};

/// A full report: every fact included. Without host data (e.g. `/proc` unreadable) the
/// host facts are absent and the host state is empty; `Collected::errors` says why.
pub fn build(c: &Collected, host: &str, now: Timestamp) -> Report {
    let (host_facts, host_state) = match &c.host {
        Some((facts, state)) => (Some(facts.clone()), state.clone()),
        None => (None, HostState::default()),
    };
    let host_state = HostState { transient_containers: c.transient.clone(), ..host_state };
    Report {
        host: host.to_string(),
        ts: now,
        agent_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        host_facts_hash: host_facts.as_ref().map(facts_hash).unwrap_or_default(),
        host_facts,
        host_state,
        workloads: c
            .workloads
            .iter()
            .map(|w| WorkloadReport {
                key: w.key.clone(),
                facts_hash: facts_hash(&w.facts),
                facts: Some(w.facts.clone()),
                state: w.state.clone(),
            })
            .collect(),
        local_events: c.local_events.clone(),
        exceptions: c.exceptions.clone(),
        errors: c.errors.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::Workload;
    use skym_core::model::TransientCounts;
    use skym_core::report::validate;

    fn collected() -> Collected {
        let r: Report = skym_core::fixtures::full_report();
        Collected {
            host: Some((r.host_facts.clone().unwrap(), r.host_state.clone())),
            workloads: r
                .workloads
                .iter()
                .map(|w| Workload {
                    key: w.key.clone(),
                    facts: w.facts.clone().unwrap(),
                    state: w.state.clone(),
                })
                .collect(),
            transient: TransientCounts { running: 2, exited: 5 },
            local_events: vec![],
            exceptions: r.exceptions.clone(),
            errors: vec![],
            all_failed: false,
            ..Default::default()
        }
    }

    #[test]
    fn full_report_is_valid_and_hashed() {
        let c = collected();
        let r = build(&c, "x", Timestamp::from_second(1_790_000_000).unwrap());
        assert_eq!(validate(&r), Ok(()));
        assert_eq!(r.host_facts_hash, facts_hash(&c.host.as_ref().unwrap().0));
        assert!(
            r.workloads
                .iter()
                .all(|w| w.facts.as_ref().map(facts_hash) == Some(w.facts_hash.clone()))
        );
        assert_eq!(r.host_state.transient_containers, TransientCounts { running: 2, exited: 5 });
    }

    #[test]
    fn without_host_data_the_facts_are_absent_but_counts_remain() {
        let c = Collected { host: None, ..collected() };
        let r = build(&c, "x", Timestamp::from_second(1_790_000_000).unwrap());
        assert_eq!((r.host_facts, r.host_facts_hash.as_str()), (None, ""));
        assert_eq!(r.host_state.transient_containers.exited, 5);
    }
}
