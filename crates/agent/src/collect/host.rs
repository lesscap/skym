//! Host facts and state from `/proc`, `/etc/os-release` and `statvfs`.

use super::split;
use anyhow::Context;
use jiff::Timestamp;
use skym_core::model::{HostFacts, HostState, MountFacts, MountState, TransientCounts};
use std::collections::{BTreeMap, BTreeSet};

/// Real block file systems. Network file systems are left out on purpose:
/// `statvfs` on a dead NFS mount hangs.
const DISK_FS: &[&str] =
    &["ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "jfs", "bcachefs", "vfat"];

pub fn read(
    hostname: &str,
    docker_root: Option<&str>,
    docker_version: Option<String>,
) -> anyhow::Result<(HostFacts, HostState, Vec<String>)> {
    let proc = |name: &str| {
        std::fs::read_to_string(format!("/proc/{name}")).with_context(|| format!("/proc/{name}"))
    };
    let (load_1m, load_5m, load_15m) = parse_loadavg(&proc("loadavg")?).context("loadavg")?;
    let (memory_total_bytes, memory_used_bytes) =
        parse_meminfo(&proc("meminfo")?).context("meminfo")?;
    let (boot_time, cpu_count) = parse_stat(&proc("stat")?).context("stat")?;
    let os = std::fs::read_to_string("/etc/os-release").ok().and_then(|s| parse_os_release(&s));
    let mounts = real_mounts(&proc("mounts")?, docker_root);
    let facts = HostFacts {
        hostname: hostname.to_string(),
        os: os.unwrap_or_default(),
        kernel: proc("sys/kernel/osrelease")?.trim().to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_count,
        memory_total_bytes,
        boot_time,
        docker_version,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        mounts: mounts.clone(),
        public_ip: None,
    };
    let (mounts, errors) =
        split(mounts.iter().map(|m| statvfs(&m.path).map_err(|e| format!("host: {e:#}"))));
    let state = HostState {
        load_1m,
        load_5m,
        load_15m,
        memory_used_bytes,
        mounts,
        transient_containers: TransientCounts::default(),
        ..HostState::default()
    };
    Ok((facts, state, errors))
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "localhost".to_string())
}

fn statvfs(path: &str) -> anyhow::Result<MountState> {
    let s = rustix::fs::statvfs(path).with_context(|| format!("statvfs {path}"))?;
    let vfs = Vfs {
        frsize: s.f_frsize,
        blocks: s.f_blocks,
        bfree: s.f_bfree,
        bavail: s.f_bavail,
        files: s.f_files,
        ffree: s.f_ffree,
    };
    Ok(mount_state(path, &vfs))
}

struct Vfs {
    frsize: u64,
    blocks: u64,
    bfree: u64,
    bavail: u64,
    files: u64,
    ffree: u64,
}

/// `df` semantics: blocks reserved for root count neither as used nor as available.
fn mount_state(path: &str, v: &Vfs) -> MountState {
    let used = v.blocks.saturating_sub(v.bfree);
    MountState {
        path: path.to_string(),
        total_bytes: (used + v.bavail) * v.frsize,
        used_bytes: used * v.frsize,
        inodes_total: v.files,
        inodes_used: v.files.saturating_sub(v.ffree),
    }
}

/// The kernel's count of OOM kills since boot, from `/proc/vmstat`.
pub fn oom_kills(vmstat: &str) -> Option<u64> {
    vmstat.lines().find_map(|l| l.strip_prefix("oom_kill ")?.trim().parse().ok())
}

pub fn parse_loadavg(s: &str) -> Option<(f64, f64, f64)> {
    let mut it = s.split_whitespace().map(str::parse::<f64>);
    Some((it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?))
}

/// `(MemTotal, MemTotal − MemAvailable)` in bytes.
pub fn parse_meminfo(s: &str) -> Option<(u64, u64)> {
    let kb: BTreeMap<&str, u64> = s
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k, v.trim().trim_end_matches(" kB").parse().ok()?))
        })
        .collect();
    let (total, available) = (*kb.get("MemTotal")?, *kb.get("MemAvailable")?);
    Some((total * 1024, total.saturating_sub(available) * 1024))
}

/// `(boot time, CPU count)`.
pub fn parse_stat(s: &str) -> Option<(Timestamp, u32)> {
    let btime = s.lines().find_map(|l| l.strip_prefix("btime "))?.trim().parse().ok()?;
    let cpus = s.lines().filter(|l| l.starts_with("cpu") && !l.starts_with("cpu ")).count();
    Some((Timestamp::from_second(btime).ok()?, cpus as u32))
}

pub fn parse_os_release(s: &str) -> Option<String> {
    let value = s.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))?;
    Some(value.trim_matches('"').to_string())
}

/// Disks worth watching: block file systems, not under the Docker root, one path per device.
pub fn real_mounts(proc_mounts: &str, docker_root: Option<&str>) -> Vec<MountFacts> {
    let under_docker = |path: &str| {
        docker_root
            .is_some_and(|root| path.starts_with(&format!("{}/", root.trim_end_matches('/'))))
    };
    let by_device = proc_mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some((f.next()?, unescape(f.next()?), f.next()?))
        })
        .filter(|(_, path, fs)| DISK_FS.contains(fs) && !under_docker(path))
        .fold(BTreeMap::<&str, MountFacts>::new(), |mut acc, (dev, path, fs)| {
            let shorter =
                acc.get(dev).is_none_or(|m| (path.len(), &path) < (m.path.len(), &m.path));
            if shorter {
                acc.insert(dev, MountFacts { path, fs_type: fs.to_string() });
            }
            acc
        });
    let mut mounts: Vec<MountFacts> = by_device.into_values().collect();
    mounts.sort_by(|a, b| a.path.cmp(&b.path));
    mounts
}

/// CPU time since boot from `/proc/stat`'s `cpu` line, in jiffies. Only the first eight
/// fields: guest time is already part of user and nice.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct CpuTimes {
    /// user, nice, system, irq, softirq.
    pub busy: u64,
    pub idle: u64,
    pub iowait: u64,
    pub steal: u64,
}

/// Received and sent bytes per interface.
pub type NetMap = BTreeMap<String, (u64, u64)>;

/// Counters the kernel keeps, each with the uptime (centiseconds) it was read at; rates
/// come from two readings.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Counters {
    pub cpu: Option<(u64, CpuTimes)>,
    /// Physical interfaces.
    pub net: Option<(u64, NetMap)>,
}

/// The counters, each on its own: one that cannot be read or parsed leaves only itself
/// out, and is named in the returned warnings. A host without a physical interface simply
/// has no network rate.
pub fn counters() -> (Counters, Vec<String>) {
    let mut warnings = Vec::new();
    let at = read_parsed("/proc/uptime", parse_uptime, &mut warnings);
    let cpu = read_parsed("/proc/stat", parse_cpu_times, &mut warnings);
    let physical = physical_interfaces();
    let net = match physical.is_empty() {
        true => None,
        false => read_parsed("/proc/net/dev", |s| Some(parse_net_dev(s, &physical)), &mut warnings),
    };
    (Counters { cpu: at.zip(cpu), net: at.zip(net) }, warnings)
}

/// A kernel file, parsed; what went wrong goes to `warnings`.
fn read_parsed<T>(
    path: &str,
    parse: impl Fn(&str) -> Option<T>,
    warnings: &mut Vec<String>,
) -> Option<T> {
    let parsed = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|s| parse(&s).ok_or_else(|| "unexpected format".to_string()));
    parsed.map_err(|e| warnings.push(format!("host: {path}: {e}"))).ok()
}

/// Interfaces backed by a device: real and virtio NICs, bond members; not bonds, bridges,
/// veths or tunnels, so traffic is counted once.
fn physical_interfaces() -> BTreeSet<String> {
    let Ok(dir) = std::fs::read_dir("/sys/class/net") else { return BTreeSet::new() };
    dir.flatten()
        .filter(|e| e.path().join("device").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// `/proc/uptime`'s first figure, in centiseconds: a clock that never steps.
pub fn parse_uptime(s: &str) -> Option<u64> {
    let (secs, frac) = s.split_whitespace().next()?.split_once('.')?;
    let frac: u64 = frac.get(..2)?.parse().ok()?;
    secs.parse::<u64>().ok()?.checked_mul(100)?.checked_add(frac)
}

pub fn parse_cpu_times(stat: &str) -> Option<CpuTimes> {
    let line = stat.lines().find_map(|l| l.strip_prefix("cpu "))?;
    let f: Vec<u64> =
        line.split_whitespace().take(8).map(|v| v.parse().ok()).collect::<Option<_>>()?;
    if f.len() < 4 {
        return None;
    }
    let at = |i: usize| f.get(i).copied().unwrap_or(0);
    Some(CpuTimes {
        busy: [0, 1, 2, 5, 6].into_iter().map(at).fold(0, u64::saturating_add),
        idle: at(3),
        iowait: at(4),
        steal: at(7),
    })
}

/// `/proc/net/dev`, for the `physical` interfaces. A large counter may run into the name
/// (`eth0:123…`), so the name ends at the first `:`.
pub fn parse_net_dev(s: &str, physical: &BTreeSet<String>) -> NetMap {
    s.lines()
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let name = name.trim();
            let f: Vec<&str> = rest.split_whitespace().collect();
            let (rx, tx) = (f.first()?.parse().ok()?, f.get(8)?.parse().ok()?);
            physical.contains(name).then(|| (name.to_string(), (rx, tx)))
        })
        .collect()
}

/// Busy, iowait and steal as shares of all CPU time between two readings. A counter that
/// went back (iowait may) counts as no time.
pub fn cpu_usage(prev: CpuTimes, cur: CpuTimes) -> Option<(f32, f32, f32)> {
    let d = |a: u64, b: u64| b.saturating_sub(a);
    let (busy, idle, iowait, steal) = (
        d(prev.busy, cur.busy),
        d(prev.idle, cur.idle),
        d(prev.iowait, cur.iowait),
        d(prev.steal, cur.steal),
    );
    let total = [busy, idle, iowait, steal].iter().fold(0u64, |sum, v| sum.saturating_add(*v));
    let share = |part: u64| (part as f64 * 100.0 / total as f64) as f32;
    (total > 0).then(|| (share(busy), share(iowait), share(steal)))
}

/// The centiseconds between two readings, when they can make a rate: at least a second,
/// at most ten minutes (an older average is not "now"), and not across a reboot.
pub fn span(prev_at: u64, cur_at: u64) -> Option<u64> {
    cur_at.checked_sub(prev_at).filter(|cs| (100..=60_000).contains(cs))
}

/// [`cpu_usage`] between two readings a valid [`span`] apart.
pub fn cpu_rates(prev: &(u64, CpuTimes), cur: &(u64, CpuTimes)) -> Option<(f32, f32, f32)> {
    span(prev.0, cur.0)?;
    cpu_usage(prev.1, cur.1)
}

/// Bytes per second received and sent between two readings a valid [`span`] apart, over
/// the interfaces present in both whose counters did not go back (a reset; also a 32-bit
/// counter wrapping, so such a NIC above ~70 MB/s is under-counted). `None` when no
/// interface is in both.
pub fn net_rates(prev: &(u64, NetMap), cur: &(u64, NetMap)) -> Option<(u64, u64)> {
    let elapsed = span(prev.0, cur.0)?;
    let deltas: Vec<(u64, u64)> = cur
        .1
        .iter()
        .filter_map(|(name, (r, t))| {
            let (pr, pt) = prev.1.get(name)?;
            Some((r.checked_sub(*pr)?, t.checked_sub(*pt)?))
        })
        .collect();
    let (rx, tx) = deltas
        .iter()
        .fold((0u64, 0u64), |(rx, tx), (r, t)| (rx.saturating_add(*r), tx.saturating_add(*t)));
    (!deltas.is_empty())
        .then(|| (rx.saturating_mul(100) / elapsed, tx.saturating_mul(100) / elapsed))
}

/// `/proc/mounts` escapes space, tab, newline and backslash as octal.
fn unescape(path: &str) -> String {
    [("\\040", " "), ("\\011", "\t"), ("\\012", "\n"), ("\\134", "\\")]
        .iter()
        .fold(path.to_string(), |p, (from, to)| p.replace(from, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/proc/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn oom_kill_counter_from_vmstat() {
        assert_eq!(oom_kills("pgfault 12\noom_kill 3\nnr_free 9\n"), Some(3));
        assert_eq!(oom_kills("pgfault 12\n"), None, "kernels before 4.13 have no counter");
    }

    #[test]
    fn real_mounts_keep_only_disks_outside_docker() {
        let mounts = real_mounts(&fixture("mounts"), Some("/data/docker"));
        let paths: Vec<&str> = mounts.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, ["/", "/boot/efi", "/data"]);
        let all = real_mounts(&fixture("mounts"), None);
        assert!(all.iter().any(|m| m.path == "/data/docker/volumes/big/_data"));
        assert_eq!(unescape("/srv/backup\\040copy"), "/srv/backup copy");
        let ties = "/dev/vdb /mnt/b ext4 rw 0 0\n/dev/vdb /mnt/a ext4 rw 0 0\n";
        assert_eq!(real_mounts(ties, None)[0].path, "/mnt/a", "independent of /proc order");
    }

    #[test]
    fn proc_parsers() {
        assert_eq!(parse_loadavg(&fixture("loadavg")), Some((0.52, 0.41, 0.30)));
        let (total, used) = parse_meminfo(&fixture("meminfo")).unwrap();
        assert_eq!((total, used), (16_336_356 * 1024, (16_336_356 - 6_131_284) * 1024));
        let (boot, cpus) = parse_stat(&fixture("stat")).unwrap();
        assert_eq!((boot.as_second(), cpus), (1_756_684_800, 2));
        assert_eq!(parse_os_release(&fixture("os-release")).unwrap(), "Ubuntu 24.04.2 LTS");
    }

    fn times(busy: u64, idle: u64, iowait: u64, steal: u64) -> CpuTimes {
        CpuTimes { busy, idle, iowait, steal }
    }

    #[test]
    fn cpu_times_from_the_first_eight_fields() {
        assert_eq!(parse_cpu_times(&fixture("stat")), Some(times(3030, 30300, 0, 0)));
        let guest = "cpu  10 1 20 300 4 5 6 7 500 500\n";
        assert_eq!(parse_cpu_times(guest), Some(times(42, 300, 4, 7)), "guest is in user already");
        assert_eq!(parse_cpu_times("cpu  1 2 3 4\n"), Some(times(6, 4, 0, 0)), "older kernels");
        for bad in ["", "cpu  1 2 3\n", "cpu  1 x 3 4\n", "intr 1 2 3 4\n"] {
            assert_eq!(parse_cpu_times(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn cpu_usage_shares_all_cpu_time() {
        let (busy, iowait, steal) =
            cpu_usage(times(100, 100, 10, 0), times(150, 130, 20, 10)).unwrap();
        assert_eq!((busy, iowait, steal), (50.0, 10.0, 10.0));
        let back = cpu_usage(times(100, 100, 50, 0), times(150, 150, 40, 0)).unwrap();
        assert_eq!(back, (50.0, 0.0, 0.0), "iowait going back is no time, not a reset");
        assert_eq!(cpu_usage(times(1, 1, 1, 1), times(1, 1, 1, 1)), None, "no time passed");
    }

    #[test]
    fn net_dev_counts_physical_interfaces_only_and_uptime_in_centiseconds() {
        let physical: BTreeSet<String> = ["eth0", "eth1"].map(String::from).into();
        let net = parse_net_dev(&fixture("net-dev"), &physical);
        assert_eq!(
            net,
            NetMap::from([
                ("eth0".into(), (1_582_405_696_200, 1_597_342_120_512)),
                ("eth1".into(), (12_345_678_901, 2000)),
            ]),
            "a counter run into its name still parses; lo, bonds, bridges and veths are out"
        );
        assert_eq!(parse_uptime(&fixture("uptime")), Some(6_902_755_685));
        assert_eq!(parse_uptime("12.3 4.0"), None, "centiseconds have two digits");
        assert_eq!(parse_uptime("x.12 4.0"), None);
    }

    #[test]
    fn rates_over_a_valid_span_and_interfaces_present_twice() {
        let map = |v: &[(&str, u64, u64)]| -> NetMap {
            v.iter().map(|(n, r, t)| (n.to_string(), (*r, *t))).collect()
        };
        let prev = (1000, map(&[("eth0", 1000, 2000), ("eth1", 5000, 5000)]));
        let cur = (1500, map(&[("eth0", 6000, 4000), ("eth1", 10, 10), ("eth2", 9, 9)]));
        assert_eq!(net_rates(&prev, &cur), Some((1000, 400)), "eth1 reset, eth2 new: left out");
        let half = (1500, map(&[("eth0", 6000, 4000), ("eth1", 9000, 10)]));
        assert_eq!(net_rates(&prev, &half), Some((1000, 400)), "one counter back is a reset too");
        assert_eq!(net_rates(&prev, &(1099, cur.1.clone())), None, "under a second");
        assert_eq!(net_rates(&prev, &(1100, cur.1.clone())), Some((5000, 2000)));
        assert_eq!(net_rates(&prev, &(61_000, cur.1.clone())), Some((8, 3)));
        assert_eq!(net_rates(&prev, &(61_001, cur.1.clone())), None, "over ten minutes");
        assert_eq!(net_rates(&prev, &(900, cur.1.clone())), None, "the clock went back");
        let gone = (1500, map(&[("eth9", 1, 1)]));
        assert_eq!(net_rates(&prev, &gone), None, "no interface in both: unknown, not 0");
        let (a, b) = (times(0, 0, 0, 0), times(10, 10, 0, 0));
        assert_eq!(cpu_rates(&(100, a), &(200, b)), Some((50.0, 0.0, 0.0)));
        assert_eq!(cpu_rates(&(100, a), &(199, b)), None, "under a second");
        assert_eq!(cpu_rates(&(200, a), &(100, b)), None, "a reboot");
    }

    #[test]
    fn mount_sizes_follow_df() {
        // 1000 blocks, 100 free of which 50 are available to users (50 reserved for root)
        let v = Vfs { frsize: 4096, blocks: 1000, bfree: 100, bavail: 50, files: 10, ffree: 4 };
        let m = mount_state("/", &v);
        assert_eq!((m.used_bytes, m.total_bytes), (900 * 4096, 950 * 4096));
        assert_eq!((m.inodes_used, m.inodes_total), (6, 10));
    }
}
