//! Which disks this machine has, for the panel's node status.
//!
//! The primary disk (`/`) has always been reported on its own. A node with a
//! data disk or two used to show nothing of them; this lists every real, local
//! disk filesystem so the panel can show them on hover and in the detail
//! drawer.
//!
//! The choosing is done by plain functions over the text of
//! `/proc/self/mountinfo`, compiled and tested on every platform. Only reading
//! that file and asking the kernel for a filesystem's size (`statvfs`) are
//! Linux-only, in [`collect_disks`] at the bottom.

use relay_shared::protocol::DiskUsage;
use std::collections::HashMap;

/// Most disks reported. Machines with more than this are nearly always
/// container or snapshot mounts rather than disks anyone wants listed.
pub const MAX_DISKS: usize = 16;

/// Filesystems that live on a disk of this machine and are worth listing.
/// `vfat` is mostly the EFI partition, so [`is_listed`] only lets it through
/// outside /boot. `fuseblk` is how ntfs-3g / exfat-fuse show a block device.
const DISK_FS_TYPES: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "jfs", "reiserfs", "bcachefs", "ntfs",
    "ntfs3", "exfat", "fuseblk", "vfat",
];

/// One line of `/proc/self/mountinfo`, the parts that matter here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// "major:minor" of the filesystem's device.
    pub dev: String,
    /// Where it is mounted, with `\040`-style escapes already decoded.
    pub mount: String,
    pub fs_type: String,
    /// The device (or dataset) it came from; may be "none".
    pub source: String,
    /// Mounted read-only (the per-mount `ro` option).
    pub read_only: bool,
}

/// Decode the `\NNN` octal escapes mountinfo uses for space, tab, newline and
/// backslash inside a path.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let is_oct = |b: u8| (b'0'..=b'7').contains(&b);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && is_oct(bytes[i + 1])
            && is_oct(bytes[i + 2])
            && is_oct(bytes[i + 3])
        {
            let v = (u32::from(bytes[i + 1] - b'0') << 6)
                | (u32::from(bytes[i + 2] - b'0') << 3)
                | u32::from(bytes[i + 3] - b'0');
            // A value above 255 is not a byte; keep the text as it was.
            if let Ok(b) = u8::try_from(v) {
                out.push(b);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse the text of `/proc/self/mountinfo`. Lines that do not have the
/// expected shape are skipped.
///
/// ```text
/// 36 35 98:0 /mnt1 /mnt2 rw,noatime master:1 - ext3 /dev/root rw,errors=continue
/// (1)(2)(3)   (4)   (5)      (6)      (7)   (8) (9)   (10)         (11)
/// ```
/// (7) is zero or more optional fields; (8), a lone "-", ends them.
pub fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(' ').collect();
            if f.len() < 10 {
                return None;
            }
            let sep = f.iter().skip(6).position(|t| *t == "-")? + 6;
            let fs_type = f.get(sep + 1)?;
            let source = f.get(sep + 2)?;
            Some(MountEntry {
                dev: f[2].to_string(),
                mount: unescape(f[4]),
                fs_type: (*fs_type).to_string(),
                source: unescape(source),
                read_only: f[5].split(',').any(|o| o == "ro"),
            })
        })
        .collect()
}

/// `/boot`, `/boot/efi` and anything under them: small system partitions
/// nobody wants in a disk list, whatever filesystem they use.
fn is_boot_mount(mount: &str) -> bool {
    mount == "/boot" || mount.starts_with("/boot/")
}

/// Is this mount a real, local disk filesystem worth listing?
fn is_listed(e: &MountEntry) -> bool {
    if !DISK_FS_TYPES.contains(&e.fs_type.as_str()) || is_boot_mount(&e.mount) {
        return false;
    }
    // A read-only loop device is a snap/appimage style image, not a disk.
    if e.read_only && e.source.starts_with("/dev/loop") {
        return false;
    }
    // ntfs-3g shows up as fuseblk; only trust it when it is a block device,
    // not some other FUSE daemon that could hang `statvfs`.
    if e.fs_type == "fuseblk" && !e.source.starts_with("/dev/") {
        return false;
    }
    true
}

/// What makes two mounts "the same disk". Bind mounts and btrfs subvolumes
/// share a device; btrfs gives each subvolume its own anonymous major:minor,
/// so a block-device source is compared instead; ZFS datasets share one pool's
/// space, so they are compared by pool.
fn disk_key(e: &MountEntry) -> String {
    if e.fs_type == "zfs" {
        let pool = e.source.split('/').next().unwrap_or(&e.source);
        return format!("zfs:{pool}");
    }
    if e.source.starts_with('/') {
        return format!("src:{}", e.source);
    }
    format!("dev:{}", e.dev)
}

/// The mounts to report, in report order: real disk filesystems (those for
/// which `usable` is true), one per disk — the one with the shortest mount path
/// — with `/` first and the rest by mount path.
///
/// Only what is visible counts: when something is mounted over a path that
/// already had a mount, the older one is hidden and ignored.
pub fn select_disk_mounts(
    entries: &[MountEntry],
    mut usable: impl FnMut(&MountEntry) -> bool,
) -> Vec<MountEntry> {
    let mut last_at: HashMap<&str, usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        last_at.insert(e.mount.as_str(), i);
    }

    let mut picked: Vec<(String, MountEntry)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if last_at.get(e.mount.as_str()) != Some(&i) || !is_listed(e) || !usable(e) {
            continue;
        }
        let key = disk_key(e);
        match picked.iter_mut().find(|(k, _)| *k == key) {
            None => picked.push((key, e.clone())),
            Some((_, p)) => {
                // Shortest path wins; a tie goes to the lexically first.
                if (e.mount.len(), &e.mount) < (p.mount.len(), &p.mount) {
                    *p = e.clone();
                }
            }
        }
    }
    let mut picked: Vec<MountEntry> = picked.into_iter().map(|(_, e)| e).collect();
    picked.sort_by(|a, b| (a.mount != "/", &a.mount).cmp(&(b.mount != "/", &b.mount)));
    picked
}

/// Turn mountinfo text into the disk list: choose the mounts, ask `stat` for
/// each one's (total, used) bytes, and drop those it cannot answer for or that
/// report no size. At most `max` are returned, and `stat` is not asked about
/// more than it takes to fill them.
pub fn pick_disks(
    mountinfo: &str,
    usable: impl FnMut(&MountEntry) -> bool,
    mut stat: impl FnMut(&str) -> Option<(u64, u64)>,
    max: usize,
) -> Vec<DiskUsage> {
    let mut out = Vec::new();
    for e in select_disk_mounts(&parse_mountinfo(mountinfo), usable) {
        if out.len() >= max {
            break;
        }
        if let Some((total, used)) = stat(&e.mount) {
            if total > 0 {
                out.push(DiskUsage {
                    mount: e.mount,
                    fs_type: e.fs_type,
                    total,
                    used: used.min(total),
                });
            }
        }
    }
    out
}

/// Every real local disk of this machine, `/` first. Empty when it cannot be
/// found out (not Linux, /proc unreadable); never an error, so a failure here
/// cannot hold up the rest of a status report.
///
/// Blocking: it makes a `statvfs` call per disk. Call it off the async
/// runtime's threads.
pub fn collect_disks() -> Vec<DiskUsage> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    pick_disks(
        &text,
        // A mount point that is a file (a container's bind-mounted /etc/hosts)
        // is not a disk, whatever device it lives on.
        |e| std::path::Path::new(&e.mount).is_dir(),
        platform::statvfs_usage,
        MAX_DISKS,
    )
}

/// No `statvfs` to ask off Linux (and no /proc/self/mountinfo to need it).
#[cfg(not(target_os = "linux"))]
mod platform {
    pub fn statvfs_usage(_path: &str) -> Option<(u64, u64)> {
        None
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::CString;

    /// Widen whatever integer type this target's libc uses for a statvfs
    /// count (u32 or u64 depending on the architecture) to u64. Going through
    /// a generic keeps clippy quiet on the targets where it is already u64.
    fn wide<T: Into<u64>>(v: T) -> u64 {
        v.into()
    }

    /// (total, used) bytes of the filesystem mounted at `path`, worked out the
    /// way the primary disk's figures are (sysinfo's: block size times blocks,
    /// and used = total - available to an ordinary user). None when the kernel
    /// will not say.
    pub fn statvfs_usage(path: &str) -> Option<(u64, u64)> {
        let cpath = CString::new(path).ok()?;
        // SAFETY: `st` is plain data the kernel fills in, and `cpath` is a
        // valid NUL-terminated string that outlives the call.
        let st = unsafe {
            let mut st: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(cpath.as_ptr(), &mut st) != 0 {
                return None;
            }
            st
        };
        let bsize = wide(st.f_bsize);
        let total = bsize.saturating_mul(wide(st.f_blocks));
        let avail = bsize.saturating_mul(wide(st.f_bavail));
        Some((total, total.saturating_sub(avail)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A realistic mountinfo: ext4 root, an xfs data disk, the boot partitions,
    // docker's overlay, tmpfs and the usual pseudo filesystems, snap images, a
    // btrfs disk mounted at two subvolumes (one device number each, as btrfs
    // does), network shares and a path with a space in it.
    const SAMPLE: &str = "\
22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw,errors=remount-ro
23 22 0:5 / /dev rw,nosuid,relatime shared:2 - devtmpfs udev rw,size=4000k
24 22 0:21 / /proc rw,nosuid,nodev,noexec,relatime shared:12 - proc proc rw
25 22 0:22 / /sys rw,nosuid,nodev,noexec,relatime shared:6 - sysfs sysfs rw
26 22 0:23 / /run rw,nosuid,nodev,noexec,relatime shared:5 - tmpfs tmpfs rw,size=800k,mode=755
27 22 0:24 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime shared:7 - cgroup2 cgroup2 rw
28 22 8:1 / /boot/efi rw,relatime shared:30 - vfat /dev/sda1 rw,fmask=0077
29 22 8:3 / /boot rw,relatime shared:31 - ext4 /dev/sda3 rw
30 22 8:16 / /data rw,relatime shared:40 - xfs /dev/sdb rw,attr2,inode64
31 22 7:0 / /snap/core20/1974 ro,nodev,relatime shared:50 - squashfs /dev/loop0 ro
32 22 7:1 / /snap/lxd/24061 ro,nodev,relatime shared:51 - ext4 /dev/loop1 ro
33 22 0:40 / /var/lib/docker/overlay2/abc/merged rw,relatime shared:60 - overlay overlay rw,lowerdir=/x
34 22 0:41 /@ /srv rw,relatime shared:70 - btrfs /dev/sdc rw,subvolid=256,subvol=/@
35 22 0:42 /@home /srv/home rw,relatime shared:71 - btrfs /dev/sdc rw,subvolid=257,subvol=/@home
36 22 0:43 / /mnt/nas rw,relatime shared:80 - nfs4 192.0.2.9:/export rw,vers=4.2
37 22 0:44 / /mnt/My\\040Disk rw,relatime shared:90 - ext4 /dev/sdd1 rw
38 22 8:16 /backup /mnt/backup-bind rw,relatime shared:41 - xfs /dev/sdb rw,attr2,inode64
39 22 0:45 / /mnt/share rw,relatime shared:91 - cifs //192.0.2.9/share rw
40 22 0:46 / /mnt/fuse rw,relatime shared:92 - fuse.sshfs user@host:/ rw
41 22 8:48 / /mnt/usb rw,relatime shared:93 - vfat /dev/sdd rw
";

    fn mounts(entries: Vec<MountEntry>) -> Vec<String> {
        entries.into_iter().map(|e| e.mount).collect()
    }

    fn select(text: &str) -> Vec<String> {
        mounts(select_disk_mounts(&parse_mountinfo(text), |_| true))
    }

    #[test]
    fn parses_every_line_and_decodes_escaped_paths() {
        let all = parse_mountinfo(SAMPLE);
        assert_eq!(all.len(), 20);
        let root = &all[0];
        assert_eq!(
            (
                root.dev.as_str(),
                root.mount.as_str(),
                root.fs_type.as_str()
            ),
            ("8:2", "/", "ext4")
        );
        assert_eq!(root.source, "/dev/sda2");
        assert!(!root.read_only);
        assert!(
            all.iter().any(|e| e.mount == "/mnt/My Disk"),
            "\\040 is a space"
        );
        assert!(all
            .iter()
            .any(|e| e.mount == "/snap/core20/1974" && e.read_only));
    }

    #[test]
    fn unescape_handles_space_tab_backslash_and_leaves_junk_alone() {
        assert_eq!(unescape("/a\\040b"), "/a b");
        assert_eq!(unescape("/a\\011b\\134c"), "/a\tb\\c");
        assert_eq!(unescape("/plain"), "/plain");
        // Not three octal digits, or not a byte: kept as written.
        assert_eq!(unescape("/a\\04"), "/a\\04");
        assert_eq!(unescape("/a\\777"), "/a\\777");
        assert_eq!(unescape("trailing\\"), "trailing\\");
        // Multi-byte UTF-8 passes through.
        assert_eq!(unescape("/数据"), "/数据");
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let text = "garbage\n\n1 2 3\n22 1 8:2 / / rw - ext4\n";
        assert!(parse_mountinfo(text).is_empty());
    }

    #[test]
    fn keeps_real_local_disks_and_orders_root_first_then_by_path() {
        // /srv/home is the same btrfs disk as /srv; /mnt/backup-bind is the
        // same xfs disk as /data.
        assert_eq!(
            select(SAMPLE),
            vec!["/", "/data", "/mnt/My Disk", "/mnt/usb", "/srv"]
        );
    }

    #[test]
    fn excludes_pseudo_network_boot_and_read_only_loop_mounts() {
        let picked = select(SAMPLE);
        for gone in [
            "/dev",
            "/proc",
            "/sys",
            "/run",
            "/sys/fs/cgroup",
            "/boot",
            "/boot/efi",
            "/snap/core20/1974",
            "/snap/lxd/24061",
            "/var/lib/docker/overlay2/abc/merged",
            "/mnt/nas",
            "/mnt/share",
            "/mnt/fuse",
        ] {
            assert!(
                !picked.iter().any(|m| m == gone),
                "{gone} must not be listed"
            );
        }
    }

    #[test]
    fn vfat_counts_outside_boot_and_a_writable_loop_does_too() {
        let text = "\
1 0 8:1 / /mnt/usb rw - vfat /dev/sdb1 rw
2 0 7:5 / /mnt/img rw - ext4 /dev/loop5 rw
3 0 8:2 / /bootstrap rw - ext4 /dev/sdc1 rw
4 0 8:3 / /boot/grub rw - ext4 /dev/sdd1 rw
";
        // "/bootstrap" only starts with "/boot"; it is not under it.
        assert_eq!(select(text), vec!["/bootstrap", "/mnt/img", "/mnt/usb"]);
    }

    #[test]
    fn ntfs_family_is_listed_and_fuseblk_only_on_a_block_device() {
        let text = "\
1 0 8:1 / /mnt/a rw - ntfs3 /dev/sdb1 rw
2 0 8:2 / /mnt/b rw - exfat /dev/sdc1 rw
3 0 0:50 / /mnt/c rw - fuseblk /dev/sdd1 rw
4 0 0:51 / /mnt/d rw - fuseblk somedaemon rw
5 0 8:5 / /mnt/e rw - zfs tank/data rw
6 0 8:6 / /mnt/f rw - bcachefs /dev/sdf rw
";
        assert_eq!(
            select(text),
            vec!["/mnt/a", "/mnt/b", "/mnt/c", "/mnt/e", "/mnt/f"]
        );
    }

    #[test]
    fn one_disk_mounted_twice_keeps_the_shortest_path() {
        // Same device (a bind mount): the shorter path wins even when it comes
        // later in the file.
        let text = "\
1 0 8:2 / / rw - ext4 /dev/sda2 rw
2 0 8:16 /deep/dir /mnt/long-bind-path rw - xfs /dev/sdb rw
3 0 8:16 / /data rw - xfs /dev/sdb rw
";
        assert_eq!(select(text), vec!["/", "/data"]);
    }

    #[test]
    fn the_same_device_number_without_a_block_source_is_one_disk() {
        let text = "\
1 0 8:2 / / rw - ext4 none rw
2 0 8:2 /sub /mnt/sub rw - ext4 none rw
";
        assert_eq!(select(text), vec!["/"]);
    }

    #[test]
    fn btrfs_subvolumes_with_different_device_numbers_are_one_disk() {
        let text = "\
1 0 0:30 /@ / rw - btrfs /dev/sda2 rw,subvol=/@
2 0 0:31 /@home /home rw - btrfs /dev/sda2 rw,subvol=/@home
3 0 0:32 /@var /var rw - btrfs /dev/sda2 rw,subvol=/@var
";
        assert_eq!(select(text), vec!["/"]);
    }

    #[test]
    fn zfs_datasets_of_one_pool_are_one_disk_and_pools_stay_apart() {
        let text = "\
1 0 0:30 / / rw - zfs rpool/ROOT/ubuntu rw
2 0 0:31 / /home rw - zfs rpool/home rw
3 0 0:32 / /tank rw - zfs tank rw
4 0 0:33 / /tank/media rw - zfs tank/media rw
";
        assert_eq!(select(text), vec!["/", "/tank"]);
    }

    #[test]
    fn a_mount_that_is_not_usable_does_not_hide_its_disk() {
        // A container bind-mounts a single file from the host's disk. The
        // file's mount is the shortest path but is not a directory, so the
        // disk must be listed under the path that is.
        let text = "\
1 0 8:2 /var/x/hosts /etc/hosts rw - ext4 /dev/sda2 rw
2 0 8:2 /var/data /opt/app-data rw - ext4 /dev/sda2 rw
";
        let picked = select_disk_mounts(&parse_mountinfo(text), |e| e.mount != "/etc/hosts");
        assert_eq!(mounts(picked), vec!["/opt/app-data"]);
    }

    #[test]
    fn a_container_without_a_disk_root_still_lists_its_volumes() {
        let text = "\
1 0 0:100 / / rw - overlay overlay rw
2 1 8:16 / /data rw - ext4 /dev/sdb rw
";
        assert_eq!(select(text), vec!["/data"]);
    }

    #[test]
    fn only_the_topmost_mount_on_a_path_counts() {
        let text = "\
1 0 8:2 / / rw - ext4 /dev/sda2 rw
2 0 8:16 / /data rw - xfs /dev/sdb rw
3 0 8:32 / /data rw - xfs /dev/sdc rw
4 0 0:60 / /srv rw - ext4 /dev/sdd rw
5 0 0:61 / /srv rw - tmpfs tmpfs rw
";
        let picked = select_disk_mounts(&parse_mountinfo(text), |_| true);
        // /data is the sdc disk (sdb is hidden under it); /srv is a tmpfs now.
        assert_eq!(
            picked
                .iter()
                .map(|e| (e.mount.as_str(), e.source.as_str()))
                .collect::<Vec<_>>(),
            vec![("/", "/dev/sda2"), ("/data", "/dev/sdc")]
        );
    }

    #[test]
    fn pick_disks_reports_sizes_and_drops_empty_or_unanswered_ones() {
        let text = "\
1 0 8:2 / / rw - ext4 /dev/sda2 rw
2 0 8:16 / /data rw - xfs /dev/sdb rw
3 0 8:32 / /empty rw - ext4 /dev/sdc rw
4 0 8:48 / /gone rw - ext4 /dev/sdd rw
5 0 8:64 / /odd rw - ext4 /dev/sde rw
";
        let disks = pick_disks(
            text,
            |_| true,
            |m| match m {
                "/" => Some((100, 40)),
                "/data" => Some((1000, 250)),
                "/empty" => Some((0, 0)),
                "/odd" => Some((10, 99)),
                _ => None,
            },
            MAX_DISKS,
        );
        let disk = |mount: &str, fs_type: &str, total, used| DiskUsage {
            mount: mount.into(),
            fs_type: fs_type.into(),
            total,
            used,
        };
        assert_eq!(
            disks,
            vec![
                disk("/", "ext4", 100, 40),
                disk("/data", "xfs", 1000, 250),
                // used is never reported as more than the total
                disk("/odd", "ext4", 10, 10),
            ]
        );
    }

    #[test]
    fn pick_disks_stops_at_the_limit_and_keeps_root() {
        let mut text = String::from("1 0 8:1 / / rw - ext4 /dev/sda1 rw\n");
        for i in 0..40 {
            text.push_str(&format!(
                "{} 0 9:{i} / /mnt/d{i:02} rw - ext4 /dev/sdx{i} rw\n",
                10 + i
            ));
        }
        let mut asked = 0;
        let disks = pick_disks(
            &text,
            |_| true,
            |_| {
                asked += 1;
                Some((10, 1))
            },
            MAX_DISKS,
        );
        assert_eq!(disks.len(), MAX_DISKS);
        assert_eq!(disks[0].mount, "/");
        assert_eq!(disks[1].mount, "/mnt/d00");
        assert_eq!(asked, MAX_DISKS, "no statvfs beyond what is reported");
    }

    #[test]
    fn nothing_to_report_is_an_empty_list() {
        assert!(pick_disks("", |_| true, |_| Some((1, 1)), MAX_DISKS).is_empty());
    }

    /// On Linux the real thing runs. (A container's root is an overlay, which
    /// is deliberately not listed, so only the shape is checked.)
    #[cfg(target_os = "linux")]
    #[test]
    fn collects_this_machines_disks_without_failing() {
        let disks = collect_disks();
        assert!(disks.len() <= MAX_DISKS);
        for d in &disks {
            assert!(d.total > 0 && d.used <= d.total, "{d:?}");
            assert!(!d.mount.is_empty() && !d.fs_type.is_empty());
        }
        if let Some(i) = disks.iter().position(|d| d.mount == "/") {
            assert_eq!(i, 0, "/ is listed first");
        }
    }
}
