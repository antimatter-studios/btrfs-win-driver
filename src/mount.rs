//! Btrfs mount handle and (with the `mount` feature) the WinFsp adapter.
//!
//! [`Mount`] opens a Btrfs volume -- an image file or, on Windows, a raw
//! device such as `\\.\PhysicalDriveN` -- optionally inside one partition
//! of a whole-disk image. Everything a WinFsp callback needs to answer is
//! a method here, written portably, so the tests in `tests/` exercise it
//! against real filesystems on Linux; the adapter at the bottom of the
//! file only translates those answers into WinFsp's types.
//!
//! **Read-only by default.** [`Mount::open`] uses `Filesystem::mount`,
//! which cannot write, and on such a mount WinFsp's `read_only_volume`
//! flag is set, so the cache manager refuses writes before any callback
//! sees them.
//!
//! **In-place writes with `--rw`, and nothing more.** [`Mount::open_rw`]
//! opens the volume with `Filesystem::mount_rw`, and a write is accepted
//! exactly where rust-fs-btrfs's C ABI accepts one (`fs_btrfs_write_file`,
//! which is `Filesystem::write_at`): an overwrite of existing bytes in a
//! `nodatacow` file of the default subvolume, inside its current size, on
//! unshared, uncompressed, allocated extents. Creating, growing,
//! truncating, renaming and deleting are refused, as is writing to an
//! ordinary copy-on-write file. Each refusal is the reader's, passed on,
//! so the driver never claims a write the format layer did not make.
//!
//! **A refusal comes when the file is opened, not after the write.**
//! Windows writes through its cache, so a refusal from the write callback
//! would arrive later, from the lazy writer, as a "delayed write failed"
//! after the application was told it had succeeded. So on a writable
//! mount every file that cannot be written in place carries the read-only
//! attribute ([`Mount::offers_write`]), and opening one for writing is
//! refused there and then ([`Mount::can_write`]).
//!
//! **Subvolumes are crossed.** A Btrfs directory entry can name another
//! subvolume's tree rather than an inode. Paths resolve through
//! `Filesystem::resolve_path_bytes`, which follows the kernel's rule for
//! crossing into one, so a subvolume or snapshot appears as the directory
//! it is under Linux and its contents are readable through it. A handle
//! into another subvolume is read-only, so files there are never written.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs_btrfs::inode::{Inode, INODE_NODATACOW, INODE_NODATASUM};
use fs_btrfs::subvol::{PathTarget, EMPTY_SUBVOL_DIR_OBJECTID};
use fs_btrfs::Filesystem;
use fs_core::{BlockDevice, BlockRead};
use winfsp_fs_skeleton::device::{BlockSource, FileSource};
use winfsp_fs_skeleton::partition;

/// A device over a `BlockSource` (sector-aligned raw-disk reads on
/// Windows, plain `pread` elsewhere), offset by `base` so the reader can
/// be handed one partition of a disk without knowing about the rest, and
/// reporting `len` as the device's size so an access past the partition's
/// end is refused rather than served from the next partition.
struct PartitionDevice {
    src: Arc<dyn BlockSource>,
    base: u64,
    len: u64,
    writable: bool,
}

impl PartitionDevice {
    /// `offset..offset+want` inside the partition, or the short read that
    /// says how far it got.
    fn bound(&self, offset: u64, want: usize) -> fs_core::Result<()> {
        let end = offset
            .checked_add(want as u64)
            .ok_or(fs_core::Error::ShortRead {
                offset,
                want,
                got: 0,
            })?;
        if end > self.len {
            return Err(fs_core::Error::ShortRead {
                offset,
                want,
                got: self.len.saturating_sub(offset) as usize,
            });
        }
        Ok(())
    }
}

impl BlockRead for PartitionDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        self.bound(offset, buf.len())?;
        self.src
            .read_at(self.base + offset, buf)
            .map_err(fs_core::Error::Io)
    }

    fn size_bytes(&self) -> u64 {
        self.len
    }
}

impl BlockDevice for PartitionDevice {
    fn write_at(&self, offset: u64, buf: &[u8]) -> fs_core::Result<()> {
        if !self.writable {
            return Err(fs_core::Error::ReadOnly);
        }
        // A write past the partition's end would land in the next one.
        self.bound(offset, buf.len())?;
        self.src
            .write_at(self.base + offset, buf)
            .map_err(fs_core::Error::Io)
    }

    fn flush(&self) -> fs_core::Result<()> {
        self.src.flush().map_err(fs_core::Error::Io)
    }

    fn is_writable(&self) -> bool {
        self.writable
    }
}

/// One entry of a directory listing: its name as stored, the inode it
/// resolves to (in whichever tree it lives), and whether reaching it
/// crossed into another subvolume.
pub struct Child {
    /// The name exactly as stored. Btrfs names are bytes with no
    /// encoding rule; the WinFsp adapter skips one that is not UTF-8,
    /// because Windows cannot be handed it.
    pub name: Vec<u8>,
    pub inode: Inode,
    pub subvolume: bool,
}

/// An opened Btrfs volume.
pub struct Mount {
    /// The volume, opened on its default subvolume's tree.
    pub fs: Filesystem,
    /// The device under it, kept to flush after a write.
    device: Arc<PartitionDevice>,
    /// Original image path, kept for diagnostic messages.
    #[allow(dead_code)]
    image: PathBuf,
}

impl Mount {
    /// Open a Btrfs volume read-only. `partition` selects a 1-indexed
    /// partition in a whole-disk image; `None` or `Some(0)` means "treat
    /// `image` as the volume directly".
    ///
    /// `Some(0)` is accepted as "no partition" because the auto-mount
    /// launcher passes `--part 0` unconditionally, through a fixed
    /// command line, for a disk with no partition table.
    pub fn open(image: &Path, partition: Option<usize>) -> Result<Self> {
        Self::open_with(image, partition, false)
    }

    /// Open a Btrfs volume for in-place writes (see the module docs for
    /// exactly which). Refused, with the reader's reason, for a volume
    /// rust-fs-btrfs will not write: a non-empty log tree, an unknown
    /// compat_ro feature, a seed device.
    pub fn open_rw(image: &Path, partition: Option<usize>) -> Result<Self> {
        Self::open_with(image, partition, true)
    }

    /// Open the image as a single, unpartitioned Btrfs volume, read-only.
    pub fn open_direct(image: &Path) -> Result<Self> {
        Self::open_with(image, None, false)
    }

    /// Open the Nth partition (1-indexed) inside `image`, read-only.
    pub fn open_partition(image: &Path, n: usize) -> Result<Self> {
        Self::open_with(image, Some(n), false)
    }

    fn open_with(image: &Path, partition: Option<usize>, writable: bool) -> Result<Self> {
        let src: Arc<dyn BlockSource> = Arc::new(if writable {
            FileSource::open_rw(image)?
        } else {
            FileSource::open(image)?
        });
        let (base, len, what) = match partition {
            None | Some(0) => (0, src.size(), String::new()),
            Some(n) => {
                let (base, len, kind) = partition_extent(src.as_ref(), image, n)?;
                (base, len, format!(" partition {n} ({kind})"))
            }
        };
        let device = Arc::new(PartitionDevice {
            src,
            base,
            len,
            writable,
        });
        let mounted = if writable {
            Filesystem::mount_rw(device.clone() as Arc<dyn BlockDevice>)
        } else {
            Filesystem::mount(device.clone() as Arc<dyn BlockRead>)
        };
        let fs = mounted.map_err(|e| {
            let hint = if partition.is_none() {
                partition_hint(image)
            } else {
                String::new()
            };
            anyhow!("open Btrfs at {}{what}: {e}{hint}", image.display())
        })?;
        Ok(Self {
            fs,
            device,
            image: image.to_path_buf(),
        })
    }

    /// Whether this mount accepts writes at all.
    pub fn is_writable(&self) -> bool {
        self.fs.is_writable()
    }

    /// Overwrite `data` at `offset` in the file `target`, and flush it to
    /// the device. Everything rust-fs-btrfs's in-place write refuses is
    /// refused with its error; a file inside another subvolume is refused
    /// as read-only, because that tree's handle cannot write. The whole
    /// range is written or none of it.
    pub fn write_in_place(
        &self,
        target: &PathTarget,
        offset: u64,
        data: &[u8],
    ) -> fs_btrfs::Result<usize> {
        if target.tree.is_some() {
            return Err(fs_btrfs::Error::ReadOnly);
        }
        let n = self.fs.write_at(target.inode.ino, offset, data)?;
        self.device
            .flush()
            .map_err(|e| fs_btrfs::Error::Io(e.to_string()))?;
        Ok(n)
    }

    /// Whether `inode` is offered for writing on this mount: the mount is
    /// writable, the inode belongs to the default subvolume's tree
    /// (`in_default_tree`), and it is a regular file the kernel marked
    /// `nodatacow` and `nodatasum`. Read from the inode alone, so it can
    /// mark every entry of a listing; the extent-level conditions are
    /// [`Mount::can_write`]'s.
    pub fn offers_write(&self, in_default_tree: bool, inode: &Inode) -> bool {
        const IN_PLACE: u64 = INODE_NODATACOW | INODE_NODATASUM;
        self.is_writable()
            && in_default_tree
            && inode.is_regular_file()
            && inode.flags & IN_PLACE == IN_PLACE
    }

    /// Whether every byte of the file `target` can be overwritten in place
    /// now: [`Mount::offers_write`], then rust-fs-btrfs's
    /// `can_write_in_place`, which also refuses a file with a shared,
    /// compressed, inline or unallocated extent.
    pub fn can_write(&self, target: &PathTarget) -> fs_btrfs::Result<bool> {
        if !self.offers_write(target.tree.is_none(), &target.inode) {
            return Ok(false);
        }
        self.fs.can_write_in_place(target.inode.ino)
    }

    /// Resolve a `/`-separated path, crossing into subvolumes. The
    /// returned target names the tree its inode belongs to; read it
    /// through `target.fs(&mount.fs)`.
    pub fn resolve(&self, path: &[u8]) -> fs_btrfs::Result<PathTarget> {
        self.fs.resolve_path_bytes(path)
    }

    /// The entries of the directory at `path`, `.` and `..` excluded,
    /// each with its inode read from the tree it lives in. An entry that
    /// names a subvolume is followed into it, so it carries that
    /// subvolume's top directory, as the kernel shows it.
    pub fn children(&self, path: &[u8]) -> fs_btrfs::Result<Vec<Child>> {
        let target = self.resolve(path)?;
        if !target.inode.is_dir() {
            return Err(fs_btrfs::Error::NotADirectory);
        }
        // The kernel's stand-in for a subvolume nothing references (a
        // snapshot's copy of a nested subvolume): an empty directory.
        if target.inode.ino == EMPTY_SUBVOL_DIR_OBJECTID {
            return Ok(Vec::new());
        }
        let tree = target.fs(&self.fs);
        let mut out = Vec::new();
        for entry in tree.read_dir(target.inode.ino)? {
            if entry.is_inode() {
                let inode = tree.read_inode(entry.ino)?;
                out.push(Child {
                    name: entry.name,
                    inode,
                    subvolume: false,
                });
            } else {
                let child = join(path, &entry.name);
                let inode = self.resolve(&child)?.inode;
                out.push(Child {
                    name: entry.name,
                    inode,
                    subvolume: true,
                });
            }
        }
        Ok(out)
    }

    /// Read `buf.len()` bytes of the regular file `target` at `offset`.
    /// Returns how many were read; fewer than asked means end of file.
    pub fn read_at(
        &self,
        target: &PathTarget,
        offset: u64,
        buf: &mut [u8],
    ) -> fs_btrfs::Result<usize> {
        target.fs(&self.fs).read_at(target.inode.ino, offset, buf)
    }

    /// A regular file's whole content.
    pub fn read_path(&self, path: &str) -> fs_btrfs::Result<Vec<u8>> {
        let target = self.resolve(path.as_bytes())?;
        if !target.inode.is_regular_file() {
            return Err(fs_btrfs::Error::NotAFile);
        }
        target.fs(&self.fs).read_file(target.inode.ino)
    }

    /// The volume's size and free space in bytes, as the superblock
    /// records them: `total_bytes`, and what of it `bytes_used` leaves.
    pub fn volume_totals(&self) -> (u64, u64) {
        let sb = self.fs.superblock();
        (sb.total_bytes, sb.total_bytes.saturating_sub(sb.bytes_used))
    }

    /// The volume label, or `btrfs` when it has none: Explorer shows an
    /// empty label as "Local Disk", which says nothing about the volume.
    pub fn label(&self) -> String {
        let label = self.fs.superblock().label.trim_end_matches('\0');
        if label.is_empty() {
            "btrfs".to_string()
        } else {
            label.to_string()
        }
    }

    /// Mount this filesystem on a Windows drive letter or empty
    /// directory, read-only unless it was opened with [`Mount::open_rw`].
    /// Blocks until Ctrl-C.
    ///
    /// Off Windows, or without the `mount` feature, prints why it cannot
    /// and returns an error, so a script that expected a mount does not
    /// carry on as though it had one.
    pub fn run(self, mount_point: &str) -> Result<()> {
        run_impl(self, mount_point)
    }
}

/// `dir` and `name` joined with one `/`.
fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = dir.to_vec();
    if !out.ends_with(b"/") {
        out.push(b'/');
    }
    out.extend_from_slice(name);
    out
}

#[cfg(not(all(windows, feature = "mount")))]
fn run_impl(_mount: Mount, _mount_point: &str) -> Result<()> {
    bail!("btrfs: a WinFsp mount needs Windows and a build with --features mount")
}

#[cfg(all(windows, feature = "mount"))]
fn run_impl(mount: Mount, mount_point: &str) -> Result<()> {
    winfsp_adapter::run(mount, mount_point)
}

/// The byte range of partition `n` (1-indexed) of `src`, checked to lie
/// inside the device, and its type for messages.
fn partition_extent(src: &dyn BlockSource, image: &Path, n: usize) -> Result<(u64, u64, String)> {
    let parts = partition::list_from_source(src)
        .with_context(|| format!("listing partitions in {}", image.display()))?;
    if parts.is_empty() {
        bail!("no partitions found in {}", image.display());
    }
    if n == 0 || n > parts.len() {
        bail!("--part {n} out of range (1..={})", parts.len());
    }
    let p = &parts[n - 1];
    let base = p.start_lba * 512;
    let len = p.num_sectors * 512;
    let end = base
        .checked_add(len)
        .ok_or_else(|| anyhow!("partition geometry overflows u64"))?;
    if end > src.size() {
        bail!(
            "partition {n} extends past device end: {end} > {} bytes",
            src.size()
        );
    }
    Ok((base, len, p.kind.to_string()))
}

fn partition_hint(image: &Path) -> String {
    match partition::list(image) {
        Ok(parts) if !parts.is_empty() => {
            let mut s =
                String::from("\nhint: this looks like a partitioned device. Try --part N:\n");
            for (i, p) in parts.iter().enumerate() {
                s.push_str(&format!(
                    "  {}: {} sectors @ LBA {} ({})\n",
                    i + 1,
                    p.num_sectors,
                    p.start_lba,
                    p.kind,
                ));
            }
            s
        }
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// WinFsp adapter (feature = "mount", windows only)
// ---------------------------------------------------------------------------

#[cfg(all(windows, feature = "mount"))]
mod winfsp_adapter {
    //! Bridge between WinFsp's `FileSystemContext` and [`Mount`].
    //!
    //! Path conversion: WinFsp hands over backslash-separated UTF-16
    //! (`\foo\bar`); the reader wants slash-separated bytes (`/foo/bar`).
    //!
    //! License posture: this module is the only place that links against
    //! the GPL-3.0 winfsp-rs crate, which is why this repository, like the
    //! program it builds, is GPL-3.0 (rust-fs-btrfs underneath stays MIT).

    use anyhow::{anyhow, Context, Result};
    use std::ffi::c_void;
    use widestring::U16CStr;
    use windows::Win32::Foundation::{
        NTSTATUS, STATUS_ACCESS_DENIED, STATUS_END_OF_FILE, STATUS_FILE_IS_A_DIRECTORY,
        STATUS_INVALID_DEVICE_REQUEST, STATUS_MEDIA_WRITE_PROTECTED, STATUS_NOT_A_DIRECTORY,
        STATUS_NOT_SUPPORTED, STATUS_OBJECT_NAME_INVALID, STATUS_OBJECT_NAME_NOT_FOUND,
        STATUS_OBJECT_PATH_NOT_FOUND,
    };
    use windows::Win32::Storage::FileSystem::{
        FILE_APPEND_DATA, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_WRITE_DATA,
    };
    // WideNameInfo is the trait that gives DirInfo its reset, set_name,
    // append_to_buffer and finalize_buffer.
    use winfsp::filesystem::{
        DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, ModificationDescriptor,
        OpenFileInfo, VolumeInfo, WideNameInfo,
    };
    use winfsp::host::{FileSystemHost, FineGuard, VolumeParams};
    use winfsp::Result as FspResult;
    use winfsp_sys::FILE_ACCESS_RIGHTS;

    use fs_btrfs::inode::Inode;
    use fs_btrfs::subvol::PathTarget;

    use super::Mount;

    /// IO_REPARSE_TAG_SYMLINK. Symlinks are surfaced as reparse points so
    /// Explorer shows them as links; following one is not implemented.
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

    use winfsp_fs_skeleton::translate::unix_to_filetime;

    /// `\foo\bar` (UTF-16) to `/foo/bar` (UTF-8 bytes). Empty is the root.
    fn winpath_to_unix(name: &U16CStr) -> FspResult<Vec<u8>> {
        let s = name
            .to_string()
            .map_err(|_| winfsp::FspError::from(STATUS_OBJECT_NAME_INVALID))?;
        if s.is_empty() {
            return Ok(b"/".to_vec());
        }
        Ok(s.replace('\\', "/").into_bytes())
    }

    /// Every entry is read-only except a file this mount offers for
    /// writing ([`Mount::offers_write`]), so Windows and the applications
    /// on it see which files can be written before trying, rather than
    /// learning it from a write that already reported success.
    fn file_attributes(inode: &Inode, writable: bool) -> u32 {
        let mut a = if writable {
            0
        } else {
            FILE_ATTRIBUTE_READONLY.0
        };
        if inode.is_dir() {
            a |= FILE_ATTRIBUTE_DIRECTORY.0;
        }
        if inode.is_symlink() {
            a |= FILE_ATTRIBUTE_REPARSE_POINT.0;
        }
        if a == 0 {
            a = FILE_ATTRIBUTE_NORMAL.0;
        }
        a
    }

    fn populate_file_info(inode: &Inode, info: &mut FileInfo, writable: bool) {
        info.file_attributes = file_attributes(inode, writable);
        info.reparse_tag = if inode.is_symlink() {
            IO_REPARSE_TAG_SYMLINK
        } else {
            0
        };
        info.file_size = if inode.is_dir() { 0 } else { inode.size };
        // nbytes is what the file occupies on disk; zero for one stored
        // inline in its extent item, which Windows reads as "no space".
        info.allocation_size = inode.nbytes.max(info.file_size).next_multiple_of(4096);
        info.creation_time = unix_to_filetime(inode.otime.sec, inode.otime.nsec);
        info.last_access_time = unix_to_filetime(inode.atime.sec, inode.atime.nsec);
        info.last_write_time = unix_to_filetime(inode.mtime.sec, inode.mtime.nsec);
        info.change_time = unix_to_filetime(inode.ctime.sec, inode.ctime.nsec);
        info.index_number = inode.ino;
        info.hard_links = 0;
        info.ea_size = 0;
    }

    /// A reader error as the NTSTATUS a WinFsp callback returns. Path
    /// problems read as "not found", so Explorer treats them uniformly;
    /// device and format errors stay distinct so they surface.
    fn err_to_status(err: fs_btrfs::Error) -> NTSTATUS {
        use fs_btrfs::Error as E;
        match err {
            E::NotFound | E::NotAFile => STATUS_OBJECT_NAME_NOT_FOUND,
            E::NotADirectory => STATUS_OBJECT_PATH_NOT_FOUND,
            _ => STATUS_INVALID_DEVICE_REQUEST,
        }
    }

    /// A refused write as its NTSTATUS: a read-only volume or tree reads
    /// as write-protected, a write the reader cannot make (copy-on-write
    /// file, shared or compressed extent, past the end) as not supported.
    fn write_err_to_status(err: fs_btrfs::Error) -> NTSTATUS {
        use fs_btrfs::Error as E;
        match err {
            E::ReadOnly => STATUS_MEDIA_WRITE_PROTECTED,
            E::UnsupportedFeature(_) => STATUS_NOT_SUPPORTED,
            other => err_to_status(other),
        }
    }

    /// Per-open state: the path, and the inode it resolved to with the
    /// subvolume tree that inode belongs to.
    pub struct BtrfsFileContext {
        path: Vec<u8>,
        target: PathTarget,
    }

    pub struct BtrfsContext {
        mount: Mount,
        label: String,
        totals: (u64, u64),
        read_only: bool,
    }

    impl BtrfsContext {
        fn ensure_writable(&self) -> FspResult<()> {
            if self.read_only {
                Err(STATUS_MEDIA_WRITE_PROTECTED.into())
            } else {
                Ok(())
            }
        }

        /// Whether the file `target` is offered for writing.
        fn offered(&self, target: &PathTarget) -> bool {
            self.mount
                .offers_write(target.tree.is_none(), &target.inode)
        }

        /// Refuses a file that is not offered for writing.
        fn ensure_offered(&self, target: &PathTarget) -> FspResult<()> {
            self.ensure_writable()?;
            if self.offered(target) {
                Ok(())
            } else {
                Err(STATUS_ACCESS_DENIED.into())
            }
        }

        fn resolve(&self, path: &[u8]) -> FspResult<PathTarget> {
            self.mount
                .resolve(path)
                .map_err(|e| err_to_status(e).into())
        }
    }

    impl FileSystemContext for BtrfsContext {
        type FileContext = BtrfsFileContext;

        fn get_security_by_name(
            &self,
            file_name: &U16CStr,
            _security_descriptor: Option<&mut [c_void]>,
            _resolve_reparse: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
        ) -> FspResult<FileSecurity> {
            let target = self.resolve(&winpath_to_unix(file_name)?)?;
            Ok(FileSecurity {
                reparse: false,
                sz_security_descriptor: 0,
                attributes: file_attributes(&target.inode, self.offered(&target)),
            })
        }

        fn open(
            &self,
            file_name: &U16CStr,
            _create_options: u32,
            granted_access: FILE_ACCESS_RIGHTS,
            file_info: &mut OpenFileInfo,
        ) -> FspResult<Self::FileContext> {
            let path = winpath_to_unix(file_name)?;
            let target = self.resolve(&path)?;
            // AN OPEN FOR WRITING IS ANSWERED HERE. Writes reach this driver
            // through the cache, so one refused in the write callback is
            // refused after the application was told it succeeded. Every
            // extent of the file is checked now instead, and a file that
            // cannot be overwritten in place is not opened for writing.
            if granted_access & (FILE_WRITE_DATA.0 | FILE_APPEND_DATA.0) != 0 {
                self.ensure_offered(&target)?;
                if !self.mount.can_write(&target).map_err(err_to_status)? {
                    return Err(STATUS_ACCESS_DENIED.into());
                }
            }
            populate_file_info(&target.inode, file_info.as_mut(), self.offered(&target));
            Ok(BtrfsFileContext { path, target })
        }

        fn close(&self, _context: Self::FileContext) {}

        fn get_file_info(
            &self,
            context: &Self::FileContext,
            file_info: &mut FileInfo,
        ) -> FspResult<()> {
            populate_file_info(
                &context.target.inode,
                file_info,
                self.offered(&context.target),
            );
            Ok(())
        }

        fn read(
            &self,
            context: &Self::FileContext,
            buffer: &mut [u8],
            offset: u64,
        ) -> FspResult<u32> {
            let inode = &context.target.inode;
            if inode.is_dir() {
                return Err(STATUS_FILE_IS_A_DIRECTORY.into());
            }
            if !inode.is_regular_file() {
                return Err(STATUS_INVALID_DEVICE_REQUEST.into());
            }
            if offset >= inode.size {
                return Err(STATUS_END_OF_FILE.into());
            }
            // The ranged read: WinFsp asks for a window and this reads
            // exactly that window, so a sequential scan costs the file's
            // size once rather than once per callback. Holes read as zeros.
            let take = (buffer.len() as u64).min(inode.size - offset) as usize;
            let n = self
                .mount
                .read_at(&context.target, offset, &mut buffer[..take])
                .map_err(err_to_status)?;
            Ok(n as u32)
        }

        fn read_directory(
            &self,
            context: &Self::FileContext,
            _pattern: Option<&U16CStr>,
            marker: DirMarker,
            buffer: &mut [u8],
        ) -> FspResult<u32> {
            if !context.target.inode.is_dir() {
                return Err(STATUS_NOT_A_DIRECTORY.into());
            }
            let children = self.mount.children(&context.path).map_err(err_to_status)?;
            // WinFsp resumes a listing after the last name it was given,
            // so the entries go out in one stable order: by name.
            // An entry is in the default subvolume's tree when this
            // directory is and the entry does not cross into another.
            let dir_in_default_tree = context.target.tree.is_none();
            let mut named: Vec<(String, Inode, bool)> = children
                .into_iter()
                .filter_map(|c| {
                    let in_default_tree = dir_in_default_tree && !c.subvolume;
                    String::from_utf8(c.name)
                        .ok()
                        .map(|n| (n, c.inode, in_default_tree))
                })
                .collect();
            named.sort_by(|a, b| a.0.cmp(&b.0));

            let resume_after = marker.inner_as_cstr().map(|m| m.to_string_lossy());
            let mut cursor: u32 = 0;
            let mut dir_info: DirInfo<255> = DirInfo::new();
            for (name, inode, in_default_tree) in &named {
                if let Some(after) = resume_after.as_deref() {
                    if name.as_str() <= after {
                        continue;
                    }
                }
                dir_info.reset();
                let writable = self.mount.offers_write(*in_default_tree, inode);
                populate_file_info(inode, dir_info.file_info_mut(), writable);
                if dir_info.set_name(name.as_str()).is_err() {
                    continue;
                }
                if !dir_info.append_to_buffer(buffer, &mut cursor) {
                    break;
                }
            }
            DirInfo::<255>::finalize_buffer(buffer, &mut cursor);
            Ok(cursor)
        }

        fn get_volume_info(&self, out_volume_info: &mut VolumeInfo) -> FspResult<()> {
            out_volume_info.total_size = self.totals.0;
            out_volume_info.free_size = self.totals.1;
            out_volume_info.set_volume_label(&self.label);
            Ok(())
        }

        // -----------------------------------------------------------------
        // Writes: an overwrite inside the file, nothing else. Creating,
        // overwriting-on-open, renaming and deleting stay at the trait's
        // default refusal, because the reader cannot make them.
        // -----------------------------------------------------------------

        fn write(
            &self,
            context: &Self::FileContext,
            buffer: &[u8],
            offset: u64,
            write_to_eof: bool,
            constrained_io: bool,
            file_info: &mut FileInfo,
        ) -> FspResult<u32> {
            self.ensure_writable()?;
            let inode = &context.target.inode;
            if inode.is_dir() {
                return Err(STATUS_FILE_IS_A_DIRECTORY.into());
            }
            self.ensure_offered(&context.target)?;
            // Appending grows the file, which the reader cannot do.
            if write_to_eof {
                return Err(STATUS_NOT_SUPPORTED.into());
            }
            let size = inode.size;
            let mut len = buffer.len() as u64;
            if constrained_io {
                // Paging I/O never extends a file: clamp to its end, and
                // a page wholly past it writes nothing.
                if offset >= size {
                    populate_file_info(inode, file_info, true);
                    return Ok(0);
                }
                len = len.min(size - offset);
            } else if offset.saturating_add(len) > size {
                return Err(STATUS_NOT_SUPPORTED.into());
            }
            let n = self
                .mount
                .write_in_place(&context.target, offset, &buffer[..len as usize])
                .map_err(write_err_to_status)?;
            populate_file_info(inode, file_info, true);
            Ok(n as u32)
        }

        fn set_file_size(
            &self,
            context: &Self::FileContext,
            new_size: u64,
            set_allocation_size: bool,
            file_info: &mut FileInfo,
        ) -> FspResult<()> {
            self.ensure_offered(&context.target)?;
            let inode = &context.target.inode;
            // The cache manager sets the size it already has, and asks for
            // allocation at least that large; both change nothing on disk.
            // Any real change of length is a truncate or an extend.
            let unchanged = if set_allocation_size {
                new_size >= inode.size
            } else {
                new_size == inode.size
            };
            if !unchanged {
                return Err(STATUS_NOT_SUPPORTED.into());
            }
            populate_file_info(inode, file_info, true);
            Ok(())
        }

        fn flush(
            &self,
            context: Option<&Self::FileContext>,
            file_info: &mut FileInfo,
        ) -> FspResult<()> {
            // Every write is flushed to the device before it returns.
            if let Some(context) = context {
                populate_file_info(
                    &context.target.inode,
                    file_info,
                    self.offered(&context.target),
                );
            }
            Ok(())
        }

        fn set_basic_info(
            &self,
            context: &Self::FileContext,
            _file_attributes: u32,
            _creation_time: u64,
            _last_access_time: u64,
            _last_write_time: u64,
            _last_change_time: u64,
            file_info: &mut FileInfo,
        ) -> FspResult<()> {
            // Timestamps and attributes are not written: the reader has no
            // inode update. Accepted rather than refused on a file offered
            // for writing, because Windows sets the write time after every
            // write and a refusal there fails the write that already
            // succeeded. Every other file is read-only, and says so.
            self.ensure_offered(&context.target)?;
            populate_file_info(&context.target.inode, file_info, true);
            Ok(())
        }

        fn set_security(
            &self,
            _context: &Self::FileContext,
            _security_information: u32,
            _modification_descriptor: ModificationDescriptor,
        ) -> FspResult<()> {
            // No security descriptors are stored; accepted on a writable
            // mount so applications that set one on every open do not fail.
            self.ensure_writable()
        }
    }

    /// Mount `mount` on `mount_point` (a drive letter such as `X:` or an
    /// empty directory) and block until Ctrl-C. Read-only unless the
    /// mount was opened with [`Mount::open_rw`].
    pub fn run(mount: Mount, mount_point: &str) -> Result<()> {
        let _init = winfsp::winfsp_init().context("WinFsp not installed?")?;

        let sector = mount.fs.superblock().sectorsize.min(4096) as u16;
        let read_only = !mount.is_writable();
        let ctx = BtrfsContext {
            read_only,
            label: mount.label(),
            totals: mount.volume_totals(),
            mount,
        };

        let mut params = VolumeParams::new();
        params
            .sector_size(sector)
            .sectors_per_allocation_unit(1)
            .max_component_length(255)
            .file_info_timeout(1000)
            .case_sensitive_search(true)
            .case_preserved_names(true)
            .unicode_on_disk(true)
            .read_only_volume(read_only)
            .filesystem_name("btrfs");

        // The locking strategy is named rather than inferred: winfsp-rs
        // carries it as a type parameter, and two impls' methods collide
        // when it is left open (E0034 at the mount() call).
        let mut host = FileSystemHost::<_, FineGuard>::new(params, ctx)
            .map_err(|e| anyhow!("FileSystemHost::new failed: {e}"))?;
        host.mount(mount_point)
            .map_err(|e| anyhow!("mount({mount_point}) failed: {e}"))?;
        host.start()
            .map_err(|e| anyhow!("FileSystemHost::start failed: {e}"))?;

        let mode = if read_only { "RO" } else { "RW, in place" };
        println!("btrfs mounted at {mount_point} ({mode}). Ctrl-C to unmount.");
        let (tx, rx) = std::sync::mpsc::channel();
        ctrlc::set_handler(move || {
            let _ = tx.send(());
        })
        .ok();
        let _ = rx.recv();

        host.stop();
        host.unmount();
        Ok(())
    }
}
