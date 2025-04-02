mod dir;
mod file;
mod inode_table;
mod symlink;

use std::{
    any::Any,
    cell::RefCell,
    cmp::min,
    fmt::Debug,
    fs::{self},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

use anyhow::{Ok, Result};
use bytemuck::{Zeroable, bytes_of, checked::from_bytes};
pub use dir::*;
pub use file::*;
pub use inode_table::*;
pub use symlink::*;
use xz2::stream::{LzmaOptions, Stream};

use crate::{
    CodexFsDirent, CodexFsExtent, CodexFsFileType, CodexFsInode, CodexFsInodeUnion, addr_to_blk_id,
    addr_to_blk_off, addr_to_nid, blk_id_to_addr, blk_size_t, blk_t,
    buffer::{BufferType, get_bufmgr_mut},
    compress::{get_cmpr_mgr, get_cmpr_mgr_mut},
    gid_t, ino_t, mode_t, nid_to_inode_meta_off, nid_to_inode_off, off_t,
    sb::{get_sb, get_sb_mut},
    uid_t,
    utils::round_down,
};

pub type InodeHandle = Rc<dyn InodeOps>;

pub trait InodeFactory: Debug {
    fn from_path(path: &Path) -> Self;
    fn from_codexfs_inode(codexfs_inode: &CodexFsInode, nid: u64) -> Self;
    fn fuse_load(codexfs_inode: &CodexFsInode, nid: u64) -> Result<Rc<Self>>;
}

pub trait InodeOps: Debug {
    fn meta(&self) -> &InodeMeta;
    fn file_type(&self) -> CodexFsFileType;
    fn as_any(&self) -> &dyn Any;
}

impl dyn InodeOps {
    pub fn downcast_file_ref(&self) -> Option<&Inode<File>> {
        self.as_any().downcast_ref::<Inode<File>>()
    }

    pub fn downcast_dir_ref(&self) -> Option<&Inode<Dir>> {
        self.as_any().downcast_ref::<Inode<Dir>>()
    }
}

impl From<&Rc<dyn InodeOps>> for CodexFsInode {
    fn from(inode: &Rc<dyn InodeOps>) -> Self {
        let blk_id = if let Some(file) = inode.as_any().downcast_ref::<Inode<File>>() {
            file.itype.inner.borrow().blk_id.unwrap_or(0)
        } else {
            0
        };
        let u = if let Some(file) = inode.as_any().downcast_ref::<Inode<File>>() {
            if get_sb().compress {
                CodexFsInodeUnion {
                    blks: file.itype.inner.borrow().extents.len() as _,
                }
            } else {
                CodexFsInodeUnion {
                    blk_off: file.itype.inner.borrow().blk_off.unwrap(),
                }
            }
        } else {
            CodexFsInodeUnion::zeroed()
        };
        let size = if let Some(file) = inode.as_any().downcast_ref::<Inode<File>>() {
            file.itype.size
        } else {
            inode.meta().meta_size()
        };
        Self {
            mode: inode.meta().mode,
            nlink: inode.meta().inner.borrow().nlink,
            size,
            blk_id,
            ino: inode.meta().ino,
            uid: inode.meta().uid,
            gid: inode.meta().gid,
            u,
            reserved: [0; _],
        }
    }
}

#[derive(Debug, Default)]
pub struct Inode<T> {
    pub meta: InodeMeta,
    pub itype: T,
}

#[derive(Debug, Default)]
pub struct InodeMeta {
    pub path: Option<PathBuf>,
    pub ino: ino_t,
    pub uid: uid_t,
    pub gid: gid_t,
    pub mode: mode_t,
    pub inner: RefCell<InodeMetaInner>,
}

#[derive(Debug, Default)]
pub struct InodeMetaInner {
    pub nlink: u16, // for dir: subdir number + 2; for file: hardlink number
    pub nid: u64,
    pub meta_size: Option<u32>,
}

impl InodeMeta {
    pub fn path(&self) -> &Path {
        self.path.as_ref().unwrap()
    }

    pub fn inode_off(&self) -> u64 {
        nid_to_inode_off(self.inner.borrow().nid)
    }

    pub fn inode_meta_off(&self) -> u64 {
        nid_to_inode_meta_off(self.inner.borrow().nid)
    }

    pub fn meta_size(&self) -> u32 {
        self.inner.borrow().meta_size.unwrap()
    }

    pub fn set_meta_size(&self, size: u32) {
        self.inner.borrow_mut().meta_size = Some(size)
    }

    fn inc_nlink(&self) {
        self.inner.borrow_mut().nlink += 1
    }
}

// WARN: Parent pointers prohibited to prevent reference cycles
#[derive(Debug)]
pub struct Dentry {
    pub path: Option<PathBuf>,
    pub file_name: String,
    pub file_type: CodexFsFileType,
    pub inode: InodeHandle,
}

impl Dentry {
    fn new_path(path: &Path, inode: InodeHandle) -> Self {
        let metadata = path.symlink_metadata().unwrap();
        Dentry {
            path: Some(path.into()),
            file_name: path.file_name().unwrap().to_string_lossy().to_string(),
            file_type: metadata.file_type().into(),
            inode,
        }
    }

    fn new_name(file_name: String, inode: InodeHandle) -> Self {
        Dentry {
            path: None,
            file_name,
            file_type: { inode.file_type() },
            inode,
        }
    }
}

impl From<&Dentry> for CodexFsDirent {
    fn from(dentry: &Dentry) -> Self {
        Self {
            nid: dentry.inode.meta().inner.borrow().nid,
            nameoff: 0,
            file_type: dentry.file_type,
            reserved: 0,
        }
    }
}

fn mkfs_load_inode_dir(path: &Path) -> Result<Rc<Inode<Dir>>> {
    assert!(path.is_dir());

    let dir = Rc::new(Inode::<Dir>::from_path(path));

    for entry in fs::read_dir(path)? {
        let entry_path = entry?.path();

        let child = mkfs_load_inode(&entry_path, Some(Rc::downgrade(&dir)))?;
        let child_dentry = Dentry::new_path(&entry_path, child);

        if child_dentry.file_type.is_dir() {
            dir.meta.inc_nlink();
        }
        dir.add_dentry(child_dentry);
    }

    Ok(dir)
}

pub fn mkfs_load_inode(path: &Path, parent: Option<Weak<Inode<Dir>>>) -> Result<InodeHandle> {
    let metadata = path.symlink_metadata()?;
    let ino = metadata.ino() as _;

    let file_type = metadata.file_type().into();
    let inode = match file_type {
        CodexFsFileType::File => {
            let inode = get_inode(ino).cloned().unwrap_or_else(|| {
                let child = Inode::<File>::from_path(path);
                let inode = Rc::new(child);
                get_cmpr_mgr_mut().files.push(inode.clone());
                inode
            });
            inode.meta().inc_nlink();
            inode
        }
        CodexFsFileType::Dir => {
            let inode = mkfs_load_inode_dir(path)?;
            let parent = parent.unwrap_or_else(|| Rc::downgrade(&inode));
            inode.set_parent(parent);
            let total_dirents_size =
                (inode.itype.inner.borrow().dentries.len() + 2) * size_of::<CodexFsDirent>();
            let total_name_size: usize = 1
                + 2
                + inode
                    .itype
                    .inner
                    .borrow()
                    .dentries
                    .iter()
                    .map(|d| d.file_name.len())
                    .sum::<usize>();
            inode
                .meta
                .set_meta_size((total_dirents_size + total_name_size) as _);
            inode as _
        }
        CodexFsFileType::CharDevice => todo!(),
        CodexFsFileType::BlockDevice => todo!(),
        CodexFsFileType::Fifo => todo!(),
        CodexFsFileType::Socket => todo!(),
        CodexFsFileType::Symlink => {
            let inode = get_inode(ino).cloned().unwrap_or_else(|| {
                let child = Inode::<SymLink>::from_path(path);
                Rc::new(child)
            });
            inode.meta().inc_nlink();
            inode
        }
        CodexFsFileType::Unknown => todo!(),
    };

    if get_inode(ino).is_none() {
        get_inode_vec_mut().push(inode.clone());
        insert_inode(ino, inode.clone());
    }

    Ok(inode)
}

pub fn mkfs_balloc_inode() {
    let buf_mgr = get_bufmgr_mut();
    for inode in get_inode_vec_mut().iter() {
        let file_type = inode.file_type();
        match file_type {
            CodexFsFileType::File => {
                let inode = inode.downcast_file_ref().unwrap();
                let addr = buf_mgr.balloc(
                    (size_of::<CodexFsInode>()
                        + inode.itype.inner.borrow().extents.len() * size_of::<CodexFsExtent>())
                        as _,
                    BufferType::Inode,
                );
                inode.meta().inner.borrow_mut().nid = addr_to_nid(addr);
            }
            CodexFsFileType::Dir => {
                let addr = buf_mgr.balloc(
                    size_of::<CodexFsInode>() as u64 + inode.meta().meta_size() as u64,
                    BufferType::Inode,
                );
                inode.meta().inner.borrow_mut().nid = addr_to_nid(addr);
            }
            CodexFsFileType::CharDevice => todo!(),
            CodexFsFileType::BlockDevice => todo!(),
            CodexFsFileType::Fifo => todo!(),
            CodexFsFileType::Socket => todo!(),
            CodexFsFileType::Symlink => {
                let addr = buf_mgr.balloc(
                    size_of::<CodexFsInode>() as u64 + inode.meta().meta_size() as u64,
                    BufferType::Inode,
                );
                inode.meta().inner.borrow_mut().nid = addr_to_nid(addr);
            }
            CodexFsFileType::Unknown => todo!(),
        }
    }
}

fn mkfs_dump_codexfs_inode(inode: &InodeHandle) -> Result<()> {
    log::info!(
        "path: {}, nid: {}",
        inode.meta().path().display(),
        inode.meta().inner.borrow().nid
    );
    let codexfs_inode = CodexFsInode::from(inode);
    get_sb().write_all_at(
        bytes_of(&codexfs_inode),
        nid_to_inode_off(inode.meta().inner.borrow().nid),
    )?;
    Ok(())
}

pub fn mkfs_dump_inode_file_data_z() -> Result<()> {
    let mut goff = 0;

    let mut output = vec![0; get_sb().blksz() as usize];
    let mut it = get_cmpr_mgr().files.iter();
    let (mut off, mut inode) = {
        if let Some(next) = it.next() {
            (0, next)
        } else {
            panic!("no files to dump");
        }
    };

    while (goff as usize) < get_cmpr_mgr().file_data.len() {
        let mut stream = Stream::new_microlzma_encoder(
            &LzmaOptions::new_preset(get_cmpr_mgr().lzma_level).unwrap(),
        )?;
        let status = stream
            .process(
                &get_cmpr_mgr().file_data[(goff) as usize..],
                &mut output,
                xz2::stream::Action::Finish,
            )
            .unwrap();
        log::debug!(
            "off {}, total_in {}, total_out {}",
            goff,
            stream.total_in(),
            stream.total_out(),
        );
        let woff = get_bufmgr_mut().balloc(get_sb().blksz() as u64, BufferType::ZData);
        assert_eq!(woff, round_down(woff, get_sb().blksz() as _));
        let input_margin = get_sb().blksz() - (stream.total_out() as blk_size_t);
        log::debug!("input margin {}", input_margin);
        get_sb()
            .write_all_at(&output, woff + input_margin as u64)
            .unwrap();

        let mut frag_off = 0;
        while frag_off < stream.total_in() {
            inode
                .itype
                .inner
                .borrow_mut()
                .blk_id
                .get_or_insert(addr_to_blk_id(woff));
            log::info!(
                "path {}, blk_id {:?}",
                inode.meta.path().display(),
                inode.itype.inner.borrow().blk_id
            );
            let len = min(
                stream.total_in() - frag_off,
                off + inode.itype.size as u64 - goff,
            );
            if inode
                .push_extent((goff - off) as _, len as _, frag_off as _)
                .is_none()
            {
                let Some(next) = it.next() else {
                    goff += len;
                    break;
                };
                (off, inode) = (off + inode.itype.size as off_t, next);
            };
            goff += len;
            frag_off += len;
        }

        get_sb_mut().end_data_blk_id = addr_to_blk_id(woff);
        get_sb_mut().end_data_blk_sz = stream.total_out() as _;
        log::info!(
            "end blk id {}, end blk sz {}, total in {}",
            get_sb().end_data_blk_id,
            get_sb().end_data_blk_sz,
            stream.total_in(),
        );

        output.fill(0);
    }

    Ok(())
}

pub fn mkfs_dump_inode_file_data() -> Result<()> {
    for file in get_cmpr_mgr().files.iter() {
        let len = file.itype.inner.borrow().content.as_ref().unwrap().len();
        let addr = get_bufmgr_mut().balloc(len as _, BufferType::Data);
        get_sb().write_all_at(file.itype.inner.borrow().content.as_ref().unwrap(), addr)?;
        file.itype
            .inner
            .borrow_mut()
            .blk_id
            .get_or_insert(addr_to_blk_id(addr));
        file.itype
            .inner
            .borrow_mut()
            .blk_off
            .get_or_insert(addr_to_blk_off(addr));
    }
    Ok(())
}

pub fn mkfs_dump_inode() -> Result<()> {
    for inode in get_inode_vec_mut().iter() {
        match inode.file_type() {
            CodexFsFileType::File => {
                let inode_file = inode.downcast_file_ref().unwrap();
                let mut extents_off = inode_file.meta.inode_meta_off();
                for codexfs_extent in inode_file.itype.inner.borrow().extents.iter() {
                    get_sb().write_all_at(bytes_of(codexfs_extent), extents_off)?;
                    extents_off += size_of::<CodexFsExtent>() as u64;
                }
                mkfs_dump_codexfs_inode(inode)?;
            }
            CodexFsFileType::Dir => {
                let inode_dir = inode.downcast_dir_ref().unwrap();
                let mut dirents = Vec::new();
                let mut names = Vec::new();
                let mut nameoff = (size_of::<CodexFsDirent>()
                    * (inode_dir.itype.inner.borrow().dentries.len() + 2))
                    as u16;

                let dot_dirent = CodexFsDirent {
                    nid: inode_dir.meta.inner.borrow().nid,
                    nameoff,
                    file_type: CodexFsFileType::Dir,
                    reserved: 0,
                };
                dirents.push(dot_dirent);
                names.push(".");
                nameoff += 1;

                let dotdot_dirent = CodexFsDirent {
                    nid: inode_dir.parent().meta.inner.borrow().nid,
                    nameoff,
                    file_type: CodexFsFileType::Dir,
                    reserved: 0,
                };
                dirents.push(dotdot_dirent);
                names.push("..");
                nameoff += 2;

                {
                    let guard = inode_dir.itype.inner.borrow();
                    for dentry in guard.dentries.iter() {
                        let mut codexfs_dirent = CodexFsDirent::from(dentry);
                        codexfs_dirent.nameoff = nameoff;
                        dirents.push(codexfs_dirent);
                        names.push(&dentry.file_name);
                        nameoff += u16::try_from(dentry.file_name.len())?;
                    }

                    let mut dirent_off = inode_dir.meta.inode_meta_off();
                    for dirent in dirents {
                        get_sb().write_all_at(bytes_of(&dirent), dirent_off)?;
                        dirent_off += size_of::<CodexFsDirent>() as u64;
                    }
                    let mut name_off = dirent_off;
                    for name in names {
                        get_sb().write_all_at(name.as_bytes(), name_off)?;
                        name_off += name.len() as u64;
                    }
                    assert_eq!(
                        inode_dir.meta.inode_meta_off() + inode_dir.meta.meta_size() as u64,
                        name_off
                    );
                }

                mkfs_dump_codexfs_inode(inode)?;
            }
            CodexFsFileType::CharDevice => todo!(),
            CodexFsFileType::BlockDevice => todo!(),
            CodexFsFileType::Fifo => todo!(),
            CodexFsFileType::Socket => todo!(),
            CodexFsFileType::Symlink => {
                let link = fs::read_link(inode.meta().path())?;
                get_sb().write_all_at(
                    link.to_string_lossy().as_bytes(),
                    inode.meta().inode_meta_off(),
                )?;
                mkfs_dump_codexfs_inode(inode)?;
            }
            CodexFsFileType::Unknown => todo!(),
        }
    }

    Ok(())
}

pub fn fuse_load_inode(nid: u64) -> Result<InodeHandle> {
    let mut inode_buf = [0; size_of::<CodexFsInode>()];
    log::info!("load inode nid {nid}");
    get_sb().read_exact_at(&mut inode_buf, nid_to_inode_off(nid))?;
    let codexfs_inode: &CodexFsInode = from_bytes(&inode_buf);

    let file_type: CodexFsFileType = codexfs_inode.mode.into();
    // TODO: this check seems only for root inode
    if !file_type.is_dir() {
        if let Some(inode) = get_inode(codexfs_inode.ino) {
            return Ok(inode.clone());
        }
    }
    let inode: InodeHandle = match file_type {
        CodexFsFileType::File => Inode::<File>::fuse_load(codexfs_inode, nid)? as _,
        CodexFsFileType::Dir => Inode::<Dir>::fuse_load(codexfs_inode, nid)? as _,
        CodexFsFileType::CharDevice => todo!(),
        CodexFsFileType::BlockDevice => todo!(),
        CodexFsFileType::Fifo => todo!(),
        CodexFsFileType::Socket => todo!(),
        CodexFsFileType::Symlink => Inode::<SymLink>::fuse_load(codexfs_inode, nid)? as _,
        CodexFsFileType::Unknown => todo!(),
    };
    insert_inode(inode.meta().ino, inode.clone());

    Ok(inode)
}

pub fn fuse_read_inode_file(inode: &Inode<File>, off: u32, len: u32) -> Result<Vec<u8>> {
    log::info!("inode size {}, off {}, len {}", inode.itype.size, off, len);
    let file = &inode.itype;
    let len_left = min(len, file.size - off);
    let mut buf = vec![0; len_left as _];
    get_sb().read_exact_at(
        &mut buf,
        blk_id_to_addr(file.inner.borrow().blk_id.unwrap())
            + file.inner.borrow().blk_off.unwrap() as u64
            + off as u64,
    )?;
    Ok(buf)
}

pub fn fixup_insize(buf: &[u8]) -> usize {
    buf.iter().position(|&x| x != 0).unwrap()
}

pub fn fuse_read_inode_file_z(inode: &Inode<File>, off: u32, len: u32) -> Result<Vec<u8>> {
    const MEM_LIMIT: usize = 16 * 1024 * 1024;
    const DICT_SIZE: usize = 8 * 1024 * 1024;

    log::info!("inode size {}, off {}, len {}", inode.itype.size, off, len);

    let file = &inode.itype;
    let mut len_left = min(len, file.size - off);
    let mut buf = vec![0; len as _];
    let mut input = vec![0; get_sb().blksz() as usize];
    let mut output = Vec::with_capacity(MEM_LIMIT);

    let i = file
        .inner
        .borrow()
        .extents
        .partition_point(|&e| e.off <= off);
    for (i, e) in file.inner.borrow().extents.iter().enumerate().skip(i - 1) {
        log::debug!("i {i}, e {:?}", e);
        let blk_id = file.inner.borrow().blk_id.unwrap() + i as blk_t;
        get_sb().read_exact_at(&mut input, blk_id_to_addr(blk_id))?;
        let comp_size = if get_sb().end_data_blk_id == blk_id {
            get_sb().end_data_blk_sz
        } else {
            get_sb().blksz()
        };
        let input_margin = fixup_insize(&input);
        log::debug!(
            "blk_id {}, comp_size {}, input_margin {}",
            blk_id,
            comp_size,
            input_margin
        );
        let mut stream =
            Stream::new_microlzma_decoder(comp_size as _, MEM_LIMIT as _, false, DICT_SIZE as _)?;
        let status = stream.process_vec(
            &input[input_margin..],
            &mut output,
            xz2::stream::Action::Finish,
        )?;
        // WARN: output may contain one extra byte so that we can not depend on the
        // length of output
        log::debug!("output len {}", output.len());

        let needed_output_len = if i + 1 < file.inner.borrow().extents.len() {
            file.inner.borrow().extents[i + 1].off - file.inner.borrow().extents[i].off
        } else {
            file.size - file.inner.borrow().extents[i].off
        };
        let len_consumed = if off >= e.off {
            min(len_left, needed_output_len - (off - e.off))
        } else {
            min(len_left, needed_output_len)
        };
        log::debug!(
            "needed_output_len {}, len_consumed {}, len_left {}",
            needed_output_len,
            len_consumed,
            len_left
        );
        if off >= e.off {
            buf[..len_consumed as _].copy_from_slice(
                &output[(e.frag_off + off - e.off) as _
                    ..(e.frag_off + off - e.off + len_consumed) as _],
            );
        } else {
            assert!(e.frag_off == 0);
            buf[(e.off - off) as _..(e.off - off + len_consumed) as _]
                .copy_from_slice(&output[..len_consumed as _]);
        }
        assert!(e.off == 0 || e.frag_off == 0);
        len_left -= len_consumed;
        if len_left == 0 {
            break;
        }
        output.clear();
    }

    Ok(buf)
}

#[cfg(test)]
mod test {
    use std::{
        fs::{self, File},
        path::Path,
        rc::Rc,
    };

    use anyhow::{Ok, Result};

    use crate::{
        compress::set_cmpr_mgr,
        inode::{InodeHandle, get_inode_by_path, mkfs_load_inode},
        sb::{SuperBlock, set_sb},
    };

    #[test]
    fn check_mkfs_load_inode() -> Result<()> {
        // .
        // ├── hello.txt
        // └── subdir
        //     └── hello.txt.hardlink

        let root = Path::new("cargo-test-fs.tmp");
        let img_path = Path::new("cargo-test-img.tmp");
        let subdir = root.join("subdir");
        let hello = root.join("hello.txt");
        let hardlink = subdir.join("hello.txt.hardlink");

        if root.exists() {
            fs::remove_dir_all(root)?;
        }

        fs::create_dir(root)?;
        fs::create_dir(&subdir)?;
        fs::write(&hello, "Hello world!")?;
        fs::hard_link(&hello, &hardlink)?;

        {
            set_sb(SuperBlock::new(File::create(img_path)?, 12));
            set_cmpr_mgr(6);
            let root_inode = mkfs_load_inode(root, None)?;
            let subdir_inode = get_inode_by_path(&subdir).unwrap();
            let hello_inode = get_inode_by_path(&hello).unwrap();
            let hardlink_inode = get_inode_by_path(&hardlink).unwrap();

            let root_parent = root_inode.downcast_dir_ref().unwrap().parent() as InodeHandle;
            assert!(Rc::ptr_eq(&root_parent, &root_inode));
            assert!(Rc::ptr_eq(hello_inode, hardlink_inode));

            assert_eq!(root_inode.meta().inner.borrow().nlink, 3);
            assert_eq!(subdir_inode.meta().inner.borrow().nlink, 2);
            assert_eq!(hello_inode.meta().inner.borrow().nlink, 2);
        }

        fs::remove_dir_all(root)?;
        fs::remove_file(img_path)?;

        Ok(())
    }
}
