//! Facts change rarely: a report carries an entity's facts only when they changed, in the
//! first report after `skym` starts, and in the first report of each hour. Pure.

use jiff::Timestamp;
use skym_core::report::Report;
use skym_core::subject::Subject;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct FactsPolicy {
    /// The facts hash last put in a report, by entity.
    sent: BTreeMap<String, String>,
    hour: Option<i64>,
}

impl FactsPolicy {
    /// Drops the facts the server already has. The hashes always stay.
    pub fn strip(&mut self, r: &mut Report) {
        let hour = hour_of(r.ts);
        let resend = self.hour != Some(hour);
        self.hour = Some(hour);
        let mut sent = BTreeMap::new();
        let mut keep = |key: String, hash: &str| {
            let keep = resend || self.sent.get(&key).is_none_or(|h| h != hash);
            sent.insert(key, hash.to_string());
            keep
        };
        if r.host_facts.is_some() && !keep("host".into(), &r.host_facts_hash) {
            r.host_facts = None;
        }
        for w in &mut r.workloads {
            if w.facts.is_some()
                && !keep(Subject::Workload(w.key.clone()).to_string(), &w.facts_hash)
            {
                w.facts = None;
            }
        }
        self.sent = sent;
    }
}

fn hour_of(ts: Timestamp) -> i64 {
    ts.as_second().div_euclid(3600)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(sec: i64) -> Report {
        let mut r = skym_core::fixtures::full_report();
        r.ts = Timestamp::from_second(1_790_000_400 + sec).unwrap(); // 20 minutes into an hour
        r
    }

    fn carried(r: &Report) -> (bool, Vec<bool>) {
        (r.host_facts.is_some(), r.workloads.iter().map(|w| w.facts.is_some()).collect())
    }

    #[test]
    fn facts_go_out_first_on_change_and_every_hour() {
        let mut p = FactsPolicy::default();
        let mut first = report(0);
        p.strip(&mut first);
        assert_eq!(carried(&first), (true, vec![true, true, true]));
        let mut same = report(60);
        p.strip(&mut same);
        assert_eq!(carried(&same), (false, vec![false, false, false]));
        assert!(!same.host_facts_hash.is_empty(), "hashes always stay");
        let mut changed = report(120);
        changed.workloads[1].facts_hash = "0000000000000000".into();
        p.strip(&mut changed);
        assert_eq!(carried(&changed), (false, vec![false, true, false]));
        let mut next_hour = report(3200);
        p.strip(&mut next_hour);
        assert_eq!(carried(&next_hour), (true, vec![true, true, true]));
    }

    #[test]
    fn a_workload_that_comes_back_sends_its_facts_again() {
        let mut p = FactsPolicy::default();
        p.strip(&mut report(0));
        let mut without = report(60);
        without.workloads.truncate(1);
        p.strip(&mut without);
        let mut back = report(120);
        p.strip(&mut back);
        assert_eq!(carried(&back), (false, vec![false, true, true]));
    }
}
