//! btrfs-win-driver CLI entry point.

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use winfsp_fs_skeleton::FsBackend;

use btrfs_win_driver::{mount, probe};

struct BtrfsBackend;

impl FsBackend for BtrfsBackend {
    const FS_NAME: &'static str = "btrfs";
    const SERVICE_NAME: &'static str = "BtrfsWatcher";
    const LAUNCHER_SERVICE_CLASS: &'static str = "btrfs-mount";
    const FILE_EXTENSION: &'static str = "img";

    fn detect(bytes: &[u8]) -> bool {
        probe::is_btrfs(bytes)
    }
}

#[derive(Parser)]
#[command(
    name = "btrfs",
    about = "Browse Btrfs volumes, and mount them on Windows through WinFsp"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct ImgArg {
    /// Btrfs filesystem image or device.
    image: PathBuf,
    /// 1-indexed partition within a whole-disk image.
    #[arg(long)]
    part: Option<usize>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the superblock: label, UUID, sizes, checksum, devices.
    Info(ImgArg),
    /// List the directory at PATH (default `/`), crossing into subvolumes.
    Ls {
        #[command(flatten)]
        img: ImgArg,
        #[arg(default_value = "/")]
        path: String,
    },
    /// Print a regular file's contents to stdout.
    Cat {
        #[command(flatten)]
        img: ImgArg,
        path: String,
    },
    /// Overwrite bytes of an existing file in place, from stdin.
    ///
    /// Only what rust-fs-btrfs can write: a `nodatacow` file of the default
    /// subvolume, inside its current size, on unshared, uncompressed,
    /// allocated extents. Anything else is refused with the reason.
    Write {
        #[command(flatten)]
        img: ImgArg,
        path: String,
        /// Byte offset in the file to write at.
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Foreground auto-mount watcher (development).
    Watch,
    /// SCM service variant -- started by `sc start BtrfsWatcher`.
    #[cfg(windows)]
    Service,
    /// Mount a volume on a drive letter through WinFsp, read-only unless
    /// `--rw`. Blocks until Ctrl-C. Needs Windows and a build with
    /// `--features mount`.
    Mount {
        disk: String,
        #[arg(long)]
        drive: String,
        #[arg(long)]
        part: Option<usize>,
        /// Accept in-place overwrites of `nodatacow` files, the only
        /// writes rust-fs-btrfs makes. Creating, growing, truncating,
        /// renaming and deleting are still refused.
        #[arg(long)]
        rw: bool,
    },
}

fn open(img: &ImgArg) -> Result<mount::Mount> {
    mount::Mount::open(&img.image, img.part)
        .with_context(|| format!("opening {}", img.image.display()))
}

fn uuid(bytes: &[u8; 16]) -> String {
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn cmd_info(img: &ImgArg) -> Result<()> {
    let m = open(img)?;
    let sb = m.fs.superblock();
    println!("label            {:?}", sb.label.trim_end_matches('\0'));
    println!("uuid             {}", uuid(&sb.fsid));
    println!("generation       {}", sb.generation);
    println!("sector size      {}", sb.sectorsize);
    println!("node size        {}", sb.nodesize);
    println!("total bytes      {}", sb.total_bytes);
    println!("bytes used       {}", sb.bytes_used);
    println!("devices          {}", sb.num_devices);
    println!("checksum         {:?}", sb.csum_type);
    println!("incompat flags   0x{:016X}", sb.incompat_flags);
    println!("compat_ro flags  0x{:016X}", sb.compat_ro_flags);
    Ok(())
}

fn cmd_ls(img: &ImgArg, path: &str) -> Result<()> {
    let m = open(img)?;
    let children = m
        .children(path.as_bytes())
        .map_err(|e| anyhow!("list {path}: {e}"))?;
    for c in children {
        // Names are bytes: Btrfs neither NUL-terminates them nor requires
        // valid UTF-8.
        let name = String::from_utf8_lossy(&c.name);
        let kind = if c.subvolume {
            "Subvolume".to_string()
        } else {
            match c.inode.file_type() {
                Some(t) => format!("{t:?}"),
                None => "?".to_string(),
            }
        };
        println!("{:<10} {:>10} {}", kind, c.inode.ino, name);
    }
    Ok(())
}

fn cmd_cat(img: &ImgArg, path: &str) -> Result<()> {
    use std::io::Write;
    let m = open(img)?;
    let buf = m.read_path(path).map_err(|e| anyhow!("read {path}: {e}"))?;
    std::io::stdout().write_all(&buf)?;
    Ok(())
}

fn cmd_write(img: &ImgArg, path: &str, offset: u64) -> Result<()> {
    use std::io::Read;
    let mut data = Vec::new();
    std::io::stdin().read_to_end(&mut data)?;
    let m = mount::Mount::open_rw(&img.image, img.part)
        .with_context(|| format!("opening {} for writing", img.image.display()))?;
    let target = m
        .resolve(path.as_bytes())
        .map_err(|e| anyhow!("lookup {path}: {e}"))?;
    let n = m
        .write_in_place(&target, offset, &data)
        .map_err(|e| anyhow!("write {path} at {offset}: {e}"))?;
    println!("wrote {n} bytes to {path} at {offset}");
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Info(a) => cmd_info(&a),
        Cmd::Ls { img, path } => cmd_ls(&img, &path),
        Cmd::Cat { img, path } => cmd_cat(&img, &path),
        Cmd::Write { img, path, offset } => cmd_write(&img, &path, offset),
        Cmd::Watch => winfsp_fs_skeleton::watch::run::<BtrfsBackend>(),
        #[cfg(windows)]
        Cmd::Service => winfsp_fs_skeleton::service::run::<BtrfsBackend>(),
        Cmd::Mount {
            disk,
            drive,
            part,
            rw,
        } => {
            let path = PathBuf::from(&disk);
            let m = if rw {
                mount::Mount::open_rw(&path, part)
            } else {
                mount::Mount::open(&path, part)
            }
            .with_context(|| format!("opening Btrfs on {disk}"))?;
            m.run(&drive)
        }
    }
}
