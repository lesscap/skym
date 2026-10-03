use super::*;
use flate2::read::GzDecoder;
use std::io::Read;

fn t(sec: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + sec).unwrap()
}

#[test]
fn names_sort_as_time_and_read_back() {
    let early = Timestamp::from_second(999_999_999).unwrap(); // one digit fewer in seconds
    assert!(name(early) < name(t(0)));
    assert!(name(t(0)) < name(t(0) + SignedDuration::from_nanos(1)));
    assert_eq!(time_of(Path::new(&name(t(5)))), Some(t(5)));
}

#[test]
fn old_reports_and_the_oldest_beyond_a_day_of_them_are_dropped() {
    let files = |secs: &[i64]| secs.iter().map(|s| PathBuf::from(name(t(*s)))).collect::<Vec<_>>();
    let now = t(100_000);
    let aged = files(&[0, 100_000 - 86_401, 100_000 - 86_400, 99_000]);
    assert_eq!(expired(&aged, now), files(&[0, 100_000 - 86_401]));
    let many: Vec<i64> = (0..1442).map(|i| 99_000 + i).collect();
    assert_eq!(expired(&files(&many), now), files(&[99_000, 99_001]));
    assert!(expired(&[PathBuf::from("notes.json.gz")], now).is_empty());
}

#[test]
fn reports_are_stored_whole_and_listed_oldest_first() {
    let dir = std::env::temp_dir().join(format!("skym-outbox-{}", std::process::id()));
    let outbox = Outbox::open(&dir).unwrap();
    let mut r = skym_core::fixtures::full_report();
    for s in [20, 10] {
        r.ts = t(s);
        outbox.push(&r).unwrap();
    }
    std::fs::write(dir.join("outbox/stray.json.tmp"), "half").unwrap();
    let pending = outbox.pending().unwrap();
    assert_eq!(pending.iter().map(|p| time_of(p).unwrap()).collect::<Vec<_>>(), [t(10), t(20)]);
    let mut json = String::new();
    GzDecoder::new(std::fs::File::open(&pending[0]).unwrap()).read_to_string(&mut json).unwrap();
    assert_eq!(serde_json::from_str::<Report>(&json).unwrap().ts, t(10));
    assert_eq!(outbox.prune(t(20) + SignedDuration::from_hours(24)).unwrap(), 1);
    assert_eq!(outbox.pending().unwrap().len(), 1);
    Outbox::open(&dir).unwrap();
    assert!(!dir.join("outbox/stray.json.tmp").exists(), "half-written reports are removed");
    std::fs::remove_dir_all(&dir).unwrap();
}
