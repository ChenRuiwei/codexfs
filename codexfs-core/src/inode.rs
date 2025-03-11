mod dir;
mod file;
mod inode_table;
mod symlink;

use std::{
    any::Any,
    cell::{Ref, RefCell},
    cmp::min,
    fmt::Debug,
    fs::{self},
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

use anyhow::{Ok, Result};
use bytemuck::{bytes_of, checked::from_bytes};
pub use dir::*;
pub use file::*;
pub use inode_table::*;
pub use symlink::*;
use xz2::stream::{LzmaOptions, Stream};

use crate::{
    CODEXFS_BLKSIZ, CodexFsDirent, CodexFsExtent, CodexFsFileType, CodexFsInode,
    CodexFsInodeFormat, addr_to_blk_id, addr_to_nid, blk_id_to_addr,
    buffer::{BufferType, get_bufmgr_mut},
    compress::{get_cmpr_mgr, get_cmpr_mgr_mut},
    gid_t, ino_t, mode_t, nid_to_inode_meta_off, nid_to_inode_off,
    sb::{get_sb, get_sb_mut},
    uid_t,
    utils::{is_dot_or_dotdot, round_down},
};

pub type InodeHandle = Rc<RefCell<dyn InodeOps>>;

pub trait InodeFactory: Debug {
    fn from_path(path: &Path) -> Self;
    fn from_codexfs_inode(codexfs_inode: &CodexFsInode, nid: u64) -> Self;
}

pub trait InodeOps: Debug {
    fn meta(&self) -> &InodeMeta;
    fn meta_mut(&mut self) -> &mut InodeMeta;
    fn file_type(&self) -> CodexFsFileType;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl dyn InodeOps {
    pub fn downcast_file_ref(&self) -> Option<&Inode<File>> {
        self.as_any().downcast_ref::<Inode<File>>()
    }

    pub fn downcast_file_mut(&mut self) -> Option<&mut Inode<File>> {
        self.as_any_mut().downcast_mut::<Inode<File>>()
    }

    pub fn downcast_dir_ref(&self) -> Option<&Inode<Dir>> {
        self.as_any().downcast_ref::<Inode<Dir>>()
    }

    pub fn downcast_dir_mut(&mut self) -> Option<&mut Inode<Dir>> {
        self.as_any_mut().downcast_mut::<Inode<Dir>>()
    }
}

impl From<&Ref<'_, dyn InodeOps>> for CodexFsInode {
    fn from(inode: &Ref<'_, dyn InodeOps>) -> Self {
        let i = inode;
        let blk_id = if let Some(i) = i.as_any().downcast_ref::<Inode<File>>() {
            i.inner.blk_id.unwrap_or(0)
        } else {
            0
        };
        let blks = if let Some(i) = i.as_any().downcast_ref::<Inode<File>>() {
            i.inner.extents.len() as _
        } else {
            0
        };
        let size = if let Some(i) = i.as_any().downcast_ref::<Inode<File>>() {
            i.inner.size
        } else {
            i.meta().meta_size()
        };
        Self {
            format: CodexFsInodeFormat::CODEXFS_INODE_FLAT_PLAIN,
            mode: inode.meta().mode,
            nlink: inode.meta().nlink,
            size,
            blk_id,
            ino: inode.meta().ino,
            uid: inode.meta().uid,
            gid: inode.meta().gid,
            blks,
            reserved: [0; _],
        }
    }
}

#[derive(Debug)]
pub struct Inode<T> {
    pub meta: InodeMeta,
    pub inner: T,
}

#[derive(Debug)]
pub struct InodeMeta {
    pub path: Option<PathBuf>,

    pub ino: ino_t,
    pub uid: uid_t,
    pub gid: gid_t,
    pub mode: mode_t,
    pub nlink: u16, // for dir: subdir number + 2; for file: hardlink number

    pub nid: u64,
    pub meta_size: Option<u32>,
}

impl InodeMeta {
    pub fn path(&self) -> &Path {
        self.path.as_ref().unwrap()
    }

    pub fn inode_off(&self) -> u64 {
        nid_to_inode_off(self.nid)
    }

    pub fn inode_meta_off(&self) -> u64 {
        nid_to_inode_meta_off(self.nid)
    }

    pub fn meta_size(&self) -> u32 {
        self.meta_size.unwrap()
    }

    pub fn set_meta_size(&mut self, size: u32) {
        self.meta_size = Some(size)
    }

    fn inc_nlink(&mut self) {
        self.nlink += 1
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
            file_type: { inode.borrow().file_type() },
            inode,
        }
    }
}

impl From<&Dentry> for CodexFsDirent {
    fn from(dentry: &Dentry) -> Self {
        Self {
            nid: dentry.inode.borrow().meta().nid,
            nameoff: 0,
            file_type: dentry.file_type,
            reserved: 0,
        }
    }
}

fn mkfs_load_inode_dir(path: &Path) -> Result<Rc<RefCell<Inode<Dir>>>> {
    assert!(path.is_dir());

    let dir = Rc::new(RefCell::new(Inode::<Dir>::from_path(path)));

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();

        let child = mkfs_load_inode(&entry_path, Some(Rc::downgrade(&dir)))?;
        let child_dentry = Dentry::new_path(&entry_path, child);

        if child_dentry.file_type.is_dir() {
            dir.borrow_mut().meta.inc_nlink();
        }
        dir.borrow_mut().add_dentry(child_dentry);
    }

    Ok(dir)
}

pub fn mkfs_load_inode(
    path: &Path,
    parent: Option<Weak<RefCell<Inode<Dir>>>>,
) -> Result<InodeHandle> {
    let metadata = path.symlink_metadata()?;
    let ino = metadata.ino() as _;

    let file_type = metadata.file_type().into();
    let inode = match file_type {
        CodexFsFileType::File => {
            let inode = get_inode(ino).cloned().unwrap_or_else(|| {
                let child = Inode::<File>::from_path(path);
                let inode = Rc::new(RefCell::new(child));
                get_cmpr_mgr_mut().push_file(inode.clone()).unwrap();
                inode
            });
            inode.borrow_mut().meta_mut().inc_nlink();
            inode
        }
        CodexFsFileType::Dir => {
            let inode = mkfs_load_inode_dir(path)?;
            {
                let mut dir = inode.borrow_mut();
                let parent = parent.unwrap_or_else(|| Rc::downgrade(&inode));
                dir.set_parent(parent);
                let total_dirents_size =
                    (dir.inner.dentries.len() + 2) * size_of::<CodexFsDirent>();
                let total_name_size: usize = 1
                    + 2
                    + dir
                        .inner
                        .dentries
                        .iter()
                        .map(|d| d.file_name.len())
                        .sum::<usize>();
                dir.meta
                    .set_meta_size((total_dirents_size + total_name_size) as _);
            }
            inode as _
        }
        CodexFsFileType::CharDevice => todo!(),
        CodexFsFileType::BlockDevice => todo!(),
        CodexFsFileType::Fifo => todo!(),
        CodexFsFileType::Socket => todo!(),
        CodexFsFileType::Symlink => {
            let inode = get_inode(ino).cloned().unwrap_or_else(|| {
                let child = Inode::<SymLink>::from_path(path);
                Rc::new(RefCell::new(child))
            });
            inode.borrow_mut().meta_mut().inc_nlink();
            inode
        }
    };

    if get_inode(ino).is_none() {
        get_inode_vec_mut().inodes.push(inode.clone());
        insert_inode(ino, inode.clone());
    }

    Ok(inode)
}

pub fn mkfs_balloc_inode() {
    let buf_mgr = get_bufmgr_mut();
    for inode in get_inode_vec_mut().inodes.iter() {
        let file_type = inode.borrow().file_type();
        match file_type {
            CodexFsFileType::File => {
                let mut guard = inode.borrow_mut();
                let file = guard.downcast_file_mut().unwrap();
                let addr = buf_mgr.balloc(
                    (size_of::<CodexFsInode>()
                        + file.inner.extents.len() * size_of::<CodexFsExtent>())
                        as _,
                    BufferType::Inode,
                );
                file.meta.nid = addr_to_nid(addr);
            }
            CodexFsFileType::Dir => {
                let mut guard = inode.borrow_mut();
                let addr = buf_mgr.balloc(
                    size_of::<CodexFsInode>() as u64 + guard.meta().meta_size.unwrap() as u64,
                    BufferType::Inode,
                );
                guard.meta_mut().nid = addr_to_nid(addr);
            }
            CodexFsFileType::CharDevice => todo!(),
            CodexFsFileType::BlockDevice => todo!(),
            CodexFsFileType::Fifo => todo!(),
            CodexFsFileType::Socket => todo!(),
            CodexFsFileType::Symlink => {
                let mut guard = inode.borrow_mut();
                let addr = buf_mgr.balloc(
                    size_of::<CodexFsInode>() as u64 + guard.meta().meta_size.unwrap() as u64,
                    BufferType::Inode,
                );
                guard.meta_mut().nid = addr_to_nid(addr);
            }
        }
    }
}

fn mkfs_dump_codexfs_inode(inode: &InodeHandle) -> Result<()> {
    log::info!(
        "path: {}, nid: {}",
        inode.borrow().meta().path().display(),
        inode.borrow().meta().nid
    );
    let inode = inode.borrow();
    let codexfs_inode = CodexFsInode::from(&inode);
    get_sb()
        .img_file
        .write_all_at(bytes_of(&codexfs_inode), nid_to_inode_off(inode.meta().nid))?;
    Ok(())
}

pub fn mkfs_dump_inode_file_data() -> Result<()> {
    get_cmpr_mgr_mut().off = 0;

    let mut output = [0; CODEXFS_BLKSIZ as usize];
    let mut it = get_cmpr_mgr().files.iter();
    let (mut off, mut inode) = {
        if let Some(next) = it.next() {
            (&next.0, &next.1)
        } else {
            panic!("no files to dump");
        }
    };

    while (get_cmpr_mgr().off as usize) < get_cmpr_mgr().origin_data.len() {
        let mut stream = Stream::new_microlzma_encoder(
            &LzmaOptions::new_preset(get_cmpr_mgr().lzma_level).unwrap(),
        )?;
        let status = stream
            .process(
                &get_cmpr_mgr().origin_data[(get_cmpr_mgr().off) as usize..],
                &mut output,
                xz2::stream::Action::Finish,
            )
            .unwrap();
        log::debug!(
            "off {}, total_in {}, total_out {}",
            get_cmpr_mgr().off,
            stream.total_in(),
            stream.total_out(),
        );
        let woff = get_bufmgr_mut().balloc(CODEXFS_BLKSIZ as u64, BufferType::Data);
        assert_eq!(woff, round_down(woff, CODEXFS_BLKSIZ as _));
        get_sb().img_file.write_all_at(&output, woff).unwrap();

        let mut frag_off = 0;
        while frag_off < stream.total_in() {
            inode
                .borrow_mut()
                .inner
                .blk_id
                .get_or_insert(addr_to_blk_id(woff));
            log::info!(
                "path {}, blk_id {:?}",
                inode.borrow().meta.path().display(),
                inode.borrow().inner.blk_id
            );
            let len = min(
                stream.total_in() - frag_off,
                *off + inode.borrow().inner.size as u64 - get_cmpr_mgr().off,
            );
            if inode
                .borrow_mut()
                .push_extent((get_cmpr_mgr().off - *off) as _, len as _, frag_off as _)
                .is_none()
            {
                let Some(next) = it.next() else {
                    get_cmpr_mgr_mut().off += len;
                    break;
                };
                (off, inode) = (&next.0, &next.1);
            };
            get_cmpr_mgr_mut().off += len;
            frag_off += len;
        }

        get_sb_mut().end_data_blk_id = addr_to_blk_id(woff);
        get_sb_mut().end_data_blk_sz = stream.total_out() as _;
        log::info!(
            "end blk id {}, end blk sz {}",
            get_sb().end_data_blk_id,
            get_sb().end_data_blk_sz
        );
    }

    Ok(())
}

pub fn mkfs_dump_inode() -> Result<()> {
    let sb = get_sb();
    for inode in get_inode_vec_mut().inodes.iter() {
        let guard = inode.borrow();
        match inode.borrow().file_type() {
            CodexFsFileType::File => {
                let file = guard.downcast_file_ref().unwrap();
                let mut extents_off = file.meta.inode_meta_off();
                for codexfs_extent in file.inner.extents.iter() {
                    sb.img_file
                        .write_all_at(bytes_of(codexfs_extent), extents_off)?;
                    extents_off += size_of::<CodexFsExtent>() as u64;
                }
                mkfs_dump_codexfs_inode(inode)?;
            }
            CodexFsFileType::Dir => {
                let dir = guard.downcast_dir_ref().unwrap();
                let mut dirents = Vec::new();
                let mut names = Vec::new();
                let mut nameoff =
                    (size_of::<CodexFsDirent>() * (dir.inner.dentries.len() + 2)) as u16;

                let dot_dirent = CodexFsDirent {
                    nid: dir.meta.nid,
                    nameoff,
                    file_type: CodexFsFileType::Dir,
                    reserved: 0,
                };
                dirents.push(dot_dirent);
                names.push(".");
                nameoff += 1;

                let dotdot_dirent = CodexFsDirent {
                    nid: dir.parent().borrow().meta.nid,
                    nameoff,
                    file_type: CodexFsFileType::Dir,
                    reserved: 0,
                };
                dirents.push(dotdot_dirent);
                names.push("..");
                nameoff += 2;

                for dentry in dir.inner.dentries.iter() {
                    let mut codexfs_dirent = CodexFsDirent::from(dentry);
                    codexfs_dirent.nameoff = nameoff;
                    dirents.push(codexfs_dirent);
                    names.push(&dentry.file_name);
                    nameoff += u16::try_from(dentry.file_name.len())?;
                }

                let mut dirent_off = dir.meta.inode_meta_off();
                for dirent in dirents {
                    sb.img_file.write_all_at(bytes_of(&dirent), dirent_off)?;
                    dirent_off += size_of::<CodexFsDirent>() as u64;
                }
                let mut name_off = dirent_off;
                for name in names {
                    sb.img_file.write_all_at(name.as_bytes(), name_off)?;
                    name_off += name.len() as u64;
                }
                assert_eq!(
                    dir.meta.inode_meta_off() + dir.meta.meta_size() as u64,
                    name_off
                );

                mkfs_dump_codexfs_inode(inode)?;
            }
            CodexFsFileType::CharDevice => todo!(),
            CodexFsFileType::BlockDevice => todo!(),
            CodexFsFileType::Fifo => todo!(),
            CodexFsFileType::Socket => todo!(),
            CodexFsFileType::Symlink => {
                let link = fs::read_link(guard.meta().path())?;
                sb.img_file.write_all_at(
                    link.to_string_lossy().as_bytes(),
                    inode.borrow().meta().inode_meta_off(),
                )?;
                mkfs_dump_codexfs_inode(inode)?;
            }
        }
    }

    Ok(())
}

pub fn fuse_load_inode_file(
    nid: u64,
    codexfs_inode: &CodexFsInode,
) -> Result<Rc<RefCell<Inode<File>>>> {
    let mut inode = Inode::<File>::from_codexfs_inode(codexfs_inode, nid);
    let mut codexfs_extent_off = nid_to_inode_meta_off(nid);
    let mut codexfs_extent_buf = [0; size_of::<CodexFsExtent>()];

    log::info!("nid {nid} blks {}", { codexfs_inode.blks });
    for _ in 0..codexfs_inode.blks {
        get_sb()
            .img_file
            .read_exact_at(&mut codexfs_extent_buf, codexfs_extent_off)?;
        let extent: CodexFsExtent = *from_bytes::<CodexFsExtent>(&codexfs_extent_buf);
        log::info!("nid {nid} push extent");
        inode.inner.extents.push(extent);
        codexfs_extent_off += size_of::<CodexFsExtent>() as u64;
    }

    Ok(Rc::new(RefCell::new(inode)))
}

pub fn fuse_load_inode_dir(
    nid: u64,
    codexfs_inode: &CodexFsInode,
) -> Result<Rc<RefCell<Inode<Dir>>>> {
    let mut inode = Inode::<Dir>::from_codexfs_inode(codexfs_inode, nid);
    let dirents_start = nid_to_inode_meta_off(nid);
    let mut dirent_buf = [0; size_of::<CodexFsDirent>()];
    get_sb()
        .img_file
        .read_exact_at(&mut dirent_buf, dirents_start)?;
    let codexfs_dirent: CodexFsDirent = *from_bytes(&dirent_buf);

    let ndir = codexfs_dirent.nameoff / (size_of::<CodexFsDirent>() as u16);
    let mut dirents = Vec::new();
    for i in 0..ndir {
        get_sb().img_file.read_exact_at(
            &mut dirent_buf,
            dirents_start + (i as usize * size_of::<CodexFsDirent>()) as u64,
        )?;
        let codexfs_dirent: CodexFsDirent = *from_bytes(&dirent_buf);
        dirents.push(codexfs_dirent);
    }
    for i in 0..ndir {
        let file_name = {
            let endoff = if i != ndir - 1 {
                dirents[(i + 1) as usize].nameoff
            } else {
                inode.meta.meta_size() as _
            };
            let startoff = dirents[(i) as usize].nameoff;
            let mut name_buf = vec![0; (endoff - startoff) as usize];
            get_sb()
                .img_file
                .read_exact_at(&mut name_buf, dirents_start + startoff as u64)?;
            String::from_utf8(name_buf)?
        };
        log::debug!("{}", file_name);
        if is_dot_or_dotdot(&file_name) {
            continue;
        }
        let child_inode = fuse_load_inode(dirents[i as usize].nid)?;
        assert_eq!(
            dirents[i as usize].file_type,
            child_inode.borrow().file_type()
        );
        let child_dentry = Dentry::new_name(file_name, child_inode);
        inode.add_dentry(child_dentry);
    }
    Ok(Rc::new(RefCell::new(inode)))
}

pub fn fuse_load_inode(nid: u64) -> Result<InodeHandle> {
    let mut inode_buf = [0; size_of::<CodexFsInode>()];
    log::info!("load inode nid {nid}");
    get_sb()
        .img_file
        .read_exact_at(&mut inode_buf, nid_to_inode_off(nid))?;
    let codexfs_inode: &CodexFsInode = from_bytes(&inode_buf);

    let file_type: CodexFsFileType = codexfs_inode.mode.into();
    if !file_type.is_dir() {
        if let Some(inode) = get_inode(codexfs_inode.ino) {
            return Ok(inode.clone());
        }
    }
    let inode: InodeHandle = match file_type {
        CodexFsFileType::File => fuse_load_inode_file(nid, codexfs_inode)? as _,
        CodexFsFileType::Dir => fuse_load_inode_dir(nid, codexfs_inode)? as _,
        CodexFsFileType::CharDevice => todo!(),
        CodexFsFileType::BlockDevice => todo!(),
        CodexFsFileType::Fifo => todo!(),
        CodexFsFileType::Socket => todo!(),
        CodexFsFileType::Symlink => {
            let inode = Inode::<SymLink>::from_codexfs_inode(codexfs_inode, nid);
            Rc::new(RefCell::new(inode)) as _
        }
    };

    insert_inode(inode.borrow().meta().ino, inode.clone());
    Ok(inode)
}

pub fn fuse_read_inode_file(inode: &Inode<File>, off: u32, len: u32) -> Result<Vec<u8>> {
    const MEM_LIMIT: usize = 16 * 1024 * 1024;

    let guard = inode;
    let inner = &guard.inner;

    log::info!("off: {}, len {}", off, len);
    let len = min(len, inner.size - off);
    let mut len_left = len;
    let mut buf = Vec::new();

    log::info!("off: {}, len {}", off, len);

    let mut input = [0; CODEXFS_BLKSIZ as usize];

    log::info!("extents: {:?}", inner.extents);
    let i = inner.extents.partition_point(|&e| e.off <= off);
    log::info!("read_inode_file {}", i);
    for (i, e) in inner.extents.iter().enumerate().skip(i - 1) {
        log::info!("i {i}, e {:?}", e);

        let mut output = Vec::with_capacity(MEM_LIMIT);
        let blk_id = inner.blk_id.unwrap() + i as u32;
        get_sb()
            .img_file
            .read_exact_at(&mut input, blk_id_to_addr(blk_id))?;
        let comp_size = if get_sb().end_data_blk_id == blk_id {
            get_sb().end_data_blk_sz
        } else {
            CODEXFS_BLKSIZ
        };
        log::info!("blk_id {}, comp_size {}", blk_id, comp_size);
        let mut stream =
            Stream::new_microlzma_decoder(comp_size as _, MEM_LIMIT as _, false, 8 * 1024 * 1024)?;
        let status = stream.process_vec(&input, &mut output, xz2::stream::Action::Finish)?;

        log::info!("total_out {}", stream.total_out());
        let len_consumed = min(len_left, stream.total_out() as u32 - e.frag_off);
        buf.extend(&output[e.frag_off as _..(e.frag_off + len_consumed) as _]);
        len_left -= len_consumed;
        log::info!("output len {}", output.len());
        log::info!("output {}", String::from_utf8_lossy_owned(output));
        if len_left == 0 {
            break;
        }
    }

    Ok(buf)
}

#[cfg(test)]
mod test {
    use std::{
        cell::RefCell,
        fs::{self, File},
        path::Path,
        rc::Rc,
    };

    use anyhow::{Ok, Result};

    use crate::{
        compress::set_cmpr_mgr,
        inode::{InodeOps, get_inode_by_path, mkfs_load_inode},
        sb::set_sb,
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
            set_sb(File::create(img_path)?);
            set_cmpr_mgr(6);
            let root_inode = mkfs_load_inode(root, None)?;
            let subdir_inode = get_inode_by_path(&subdir).unwrap();
            let hello_inode = get_inode_by_path(&hello).unwrap();
            let hardlink_inode = get_inode_by_path(&hardlink).unwrap();

            let root_parent = root_inode.borrow().downcast_dir_ref().unwrap().parent()
                as Rc<RefCell<dyn InodeOps>>;
            assert!(Rc::ptr_eq(&root_parent, &root_inode));
            assert!(Rc::ptr_eq(hello_inode, hardlink_inode));

            assert_eq!(root_inode.borrow().meta().nlink, 3);
            assert_eq!(subdir_inode.borrow().meta().nlink, 2);
            assert_eq!(hello_inode.borrow().meta().nlink, 2);
        }

        fs::remove_dir_all(root)?;
        fs::remove_file(img_path)?;

        Ok(())
    }
}
