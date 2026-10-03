//! Host facts and state from `/proc`, `/etc/os-release` and `statvfs`.

use super::split;
use anyhow::Context;
use jiff::Timestamp;
use skym_core::model::{HostFacts, HostState, MountFacts, MountState, TransientCounts};
use std::collections::BTreeMap;

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

    #[test]
    fn mount_sizes_follow_df() {
        // 1000 blocks, 100 free of which 50 are available to users (50 reserved for root)
        let v = Vfs { frsize: 4096, blocks: 1000, bfree: 100, bavail: 50, files: 10, ffree: 4 };
        let m = mount_state("/", &v);
        assert_eq!((m.used_bytes, m.total_bytes), (900 * 4096, 950 * 4096));
        assert_eq!((m.inodes_used, m.inodes_total), (6, 10));
    }
}
