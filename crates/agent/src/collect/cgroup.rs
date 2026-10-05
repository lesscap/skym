//! Where a workload's cgroup counters are, on cgroup v2 or v1, and what they say. Plain
//! file reads of what the kernel keeps; the parsers are pure.

use super::containers::working_set;
use std::path::{Path, PathBuf};

const ROOT: &str = "/sys/fs/cgroup";

/// A workload's CPU time counter: v2's `cpu.stat` (`usage_usec`) or v1's `cpuacct.usage`
/// (nanoseconds). v1 has a `cpu.stat` too, without the usage.
#[derive(Clone, Debug, PartialEq)]
pub enum CpuFile {
    V2(PathBuf),
    V1(PathBuf),
}

/// A container's cgroup, under the systemd or the cgroupfs driver.
fn container_groups(id: &str) -> [String; 2] {
    [format!("system.slice/docker-{id}.scope"), format!("docker/{id}")]
}

/// A container's CPU counter.
pub fn container_cpu(id: &str) -> Option<CpuFile> {
    cpu_file(&container_groups(id))
}

/// A systemd unit's CPU counter, from its `ControlGroup` (`/system.slice/xray.service`).
pub fn unit_cpu(control_group: &str) -> Option<CpuFile> {
    let group = control_group.trim_start_matches('/');
    if group.is_empty() {
        return None;
    }
    cpu_file(&[group.to_string()])
}

fn cpu_file(groups: &[String]) -> Option<CpuFile> {
    let v2 = groups.iter().map(|g| Path::new(ROOT).join(g).join("cpu.stat")).map(CpuFile::V2);
    let v1 =
        groups.iter().map(|g| Path::new(ROOT).join("cpu,cpuacct").join(g).join("cpuacct.usage"));
    v2.chain(v1.map(CpuFile::V1)).find(|f| match f {
        CpuFile::V2(p) | CpuFile::V1(p) => p.exists(),
    })
}

/// The CPU time the counter holds, in nanoseconds.
pub fn read_ns(f: &CpuFile) -> Option<u64> {
    match f {
        CpuFile::V2(p) => usage_usec(&std::fs::read_to_string(p).ok()?)?.checked_mul(1000),
        CpuFile::V1(p) => cpuacct_ns(&std::fs::read_to_string(p).ok()?),
    }
}

/// A container's memory as `docker stats` counts it: usage minus inactive page cache.
pub fn container_memory(id: &str) -> Option<u64> {
    let read = |path: PathBuf| std::fs::read_to_string(path).ok();
    let groups = container_groups(id);
    let v2 = groups.iter().map(|g| Path::new(ROOT).join(g)).find_map(|dir| {
        let current = read(dir.join("memory.current"))?.trim().parse().ok()?;
        Some(working_set(current, &read(dir.join("memory.stat")).unwrap_or_default()))
    });
    v2.or_else(|| {
        groups.iter().map(|g| Path::new(ROOT).join("memory").join(g)).find_map(|dir| {
            let usage = read(dir.join("memory.usage_in_bytes"))?.trim().parse().ok()?;
            Some(working_set_v1(usage, &read(dir.join("memory.stat")).unwrap_or_default()))
        })
    })
}

/// v2's `cpu.stat` `usage_usec`; v1's `cpu.stat` has none.
pub fn usage_usec(cpu_stat: &str) -> Option<u64> {
    cpu_stat.lines().find_map(|l| l.strip_prefix("usage_usec ")?.trim().parse().ok())
}

/// v1's `cpuacct.usage`, in nanoseconds.
pub fn cpuacct_ns(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

/// v1 memory as `docker stats` counts it: usage minus the inactive page cache of the cgroup
/// and its children (`total_inactive_file`).
pub fn working_set_v1(usage: u64, memory_stat: &str) -> u64 {
    let inactive = memory_stat
        .lines()
        .find_map(|l| l.strip_prefix("total_inactive_file ")?.trim().parse::<u64>().ok())
        .unwrap_or(0);
    usage.saturating_sub(inactive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/cgroup/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn cpu_time_from_v2_and_v1() {
        assert_eq!(usage_usec(&fixture("v2-cpu.stat")), Some(1_113_486_830));
        assert_eq!(usage_usec(&fixture("v1-cpu.stat")), None, "v1's cpu.stat has no usage");
        assert_eq!(cpuacct_ns(&fixture("v1-cpuacct.usage")), Some(5_314_416_565_008));
        for bad in ["", "usage_usec x\n", "-1\n"] {
            assert_eq!((usage_usec(bad), cpuacct_ns(bad)), (None, None), "{bad:?}");
        }
    }

    #[test]
    fn v1_memory_leaves_out_inactive_cache() {
        let usage: u64 = fixture("v1-memory.usage_in_bytes").trim().parse().unwrap();
        assert_eq!(working_set_v1(usage, &fixture("v1-memory.stat")), 25_100_288 - 2_625_536);
        let only_own = "inactive_file 100\ntotal_inactive_file 300\n";
        assert_eq!(working_set_v1(1000, only_own), 700, "the total, children included");
        assert_eq!(working_set_v1(1000, ""), 1000);
        assert_eq!(working_set_v1(100, "total_inactive_file 300\n"), 0);
    }
}
