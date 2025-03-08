use std::{
    cell::{OnceCell, Ref, RefCell},
    cmp::{Ordering, min},
    collections::HashMap,
    fs,
    io::Read,
    ops::{Deref, DerefMut},
    os::unix::fs::{FileExt, FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

use anyhow::{Ok, Result};
use bytemuck::{bytes_of, checked::from_bytes};
use libc::{S_IFBLK, S_IFCHR, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, S_IFSOCK};
use xz2::stream::{LzmaOptions, Stream};

use crate::{
    CODEXFS_BLKSIZ, CODEXFS_BLKSIZ_BITS, CODEXFS_ISLOT_BITS, CodexFsDirent, CodexFsExtent,
    CodexFsFileType, CodexFsInode, CodexFsInodeFormat,
    buffer::{BufferType, get_bufmgr_mut},
    codexfs_blknr, codexfs_nid,
    compress::{get_cmpr_mgr, get_cmpr_mgr_mut},
    gid_t, ino_t, mode_t,
    sb::{get_sb, get_sb_mut},
    uid_t,
    utils::{is_dot_or_dotdot, round_down, round_up},
};

type InodeTable = HashMap<ino_t, Rc<RefCell<Inode>>>;

fn get_inode_table_mut() -> &'static mut InodeTable {
    static mut FILE_NODE_TABLE: OnceCell<InodeTable> = OnceCell::new();
    unsafe { FILE_NODE_TABLE.get_mut_or_init(HashMap::new) }
}

pub fn get_inode(ino: ino_t) -> Option<&'static Rc<RefCell<Inode>>> {
    get_inode_table_mut().get(&ino)
}

fn get_inode_by_path(path: &Path) -> Option<&'static Rc<RefCell<Inode>>> {
    let ino = path.symlink_metadata().unwrap().ino();
    get_inode(ino)
}

fn insert_inode(ino: ino_t, inode: Rc<RefCell<Inode>>) {
    get_inode_table_mut().insert(ino, inode);
}

pub struct InodeVec {
    pub inodes: Vec<Rc<RefCell<Inode>>>,
}

pub fn get_inode_vec_mut() -> &'static mut InodeVec {
    static mut INODE_VEC: OnceCell<InodeVec> = OnceCell::new();
    unsafe { INODE_VEC.get_mut_or_init(|| InodeVec { inodes: Vec::new() }) }
}

#[derive(Debug)]
pub enum FileType {
    File(FileMeta),
    Dir(DirMeta),
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
    Symlink,
}

#[derive(Debug, Default)]
pub struct FileMeta {
    pub blk_id: Option<u32>,
    pub extents: Vec<CodexFsExtent>,
}

#[derive(Debug, Default)]
pub struct DirMeta {
    pub parent: Option<Weak<RefCell<Inode>>>, // root points to itself
    pub dentries: Vec<Dentry>,
}

impl FileType {
    pub const fn is_file(&self) -> bool {
        matches!(self, Self::File { .. })
    }

    pub const fn is_dir(&self) -> bool {
        matches!(self, Self::Dir { .. })
    }

    pub const fn is_symlink(&self) -> bool {
        matches!(self, Self::Symlink)
    }

    pub const fn is_block_device(&self) -> bool {
        matches!(self, Self::BlockDevice)
    }

    pub const fn is_char_device(&self) -> bool {
        matches!(self, Self::CharDevice)
    }

    pub const fn is_fifo(&self) -> bool {
        matches!(self, Self::Fifo)
    }

    pub const fn is_socket(&self) -> bool {
        matches!(self, Self::Socket)
    }
}

impl From<std::fs::FileType> for FileType {
    fn from(val: std::fs::FileType) -> Self {
        if val.is_dir() {
            FileType::Dir(DirMeta::default())
        } else if val.is_file() {
            FileType::File(FileMeta::default())
        } else if val.is_char_device() {
            FileType::CharDevice
        } else if val.is_block_device() {
            FileType::BlockDevice
        } else if val.is_fifo() {
            FileType::Fifo
        } else if val.is_socket() {
            FileType::Socket
        } else if val.is_symlink() {
            FileType::Symlink
        } else {
            panic!("unknown file type")
        }
    }
}

impl From<&CodexFsInode> for FileType {
    fn from(codexfs_inode: &CodexFsInode) -> Self {
        match codexfs_inode.mode & S_IFMT {
            S_IFREG => FileType::File(FileMeta {
                blk_id: if codexfs_inode.blk_id != 0 {
                    Some(codexfs_inode.blk_id)
                } else {
                    None
                },
                ..Default::default()
            }),
            S_IFDIR => FileType::Dir(DirMeta::default()),
            S_IFCHR => FileType::CharDevice,
            S_IFBLK => FileType::BlockDevice,
            S_IFSOCK => FileType::Socket,
            S_IFLNK => FileType::Symlink,
            _ => panic!("unknown file type"),
        }
    }
}

impl From<&FileType> for CodexFsFileType {
    fn from(file_type: &FileType) -> Self {
        match file_type {
            FileType::File(_) => CodexFsFileType::File,
            FileType::Dir(_) => CodexFsFileType::Dir,
            FileType::CharDevice => CodexFsFileType::CharDevice,
            FileType::BlockDevice => CodexFsFileType::BlockDevice,
            FileType::Fifo => CodexFsFileType::Fifo,
            FileType::Socket => CodexFsFileType::Socket,
            FileType::Symlink => CodexFsFileType::Symlink,
        }
    }
}

#[derive(Debug)]
pub struct Inode {
    pub common: InodeCommon,
    pub file_type: FileType,
}

#[derive(Debug)]
pub struct InodeCommon {
    pub path: Option<PathBuf>,

    pub size: u32,
    pub ino: ino_t,
    pub uid: uid_t,
    pub gid: gid_t,
    pub mode: mode_t,
    pub nid: u64,
    pub nlink: u16, // for dir: subdir number + 2; for file: hardlink number
}

#[derive(Debug)]
pub struct Dentry {
    pub path: Option<PathBuf>,
    pub file_name: String,
    pub file_type: CodexFsFileType,
    pub inode: Rc<RefCell<Inode>>,
}

impl Inode {
    fn new(path: &Path) -> Self {
        let metadata = path.symlink_metadata().unwrap();
        log::info!("{}, size {}", path.display(), metadata.len());
        Self {
            common: InodeCommon {
                path: Some(path.into()),
                size: metadata.len() as _,
                nlink: if metadata.is_dir() { 2 } else { 0 },
                ino: get_sb_mut().get_ino_and_inc(),
                gid: metadata.gid(),
                uid: metadata.uid(),
                nid: 0,
                mode: metadata.mode(),
            },
            file_type: metadata.file_type().into(),
        }
    }

    pub fn load_from_nid(nid: u64) -> Result<Rc<RefCell<Self>>> {
        let mut inode_buf = [0; size_of::<CodexFsInode>()];
        get_sb()
            .img_file
            .read_exact_at(&mut inode_buf, nid << CODEXFS_ISLOT_BITS)?;
        let codexfs_inode: &CodexFsInode = from_bytes(&inode_buf);
        let inode = Rc::new(RefCell::new(Self::from_codexfs_inode(codexfs_inode, nid)));
        insert_inode(inode.borrow().common.ino, inode.clone());
        Ok(inode)
    }

    fn from_codexfs_inode(codexfs_inode: &CodexFsInode, nid: u64) -> Self {
        Self {
            common: InodeCommon {
                path: None,
                size: codexfs_inode.size,
                ino: codexfs_inode.ino,
                uid: codexfs_inode.uid,
                gid: codexfs_inode.gid,
                mode: codexfs_inode.mode,
                nid,
                nlink: codexfs_inode.nlink,
            },
            file_type: FileType::from(codexfs_inode),
        }
    }

    pub fn get_file_meta(&self) -> &FileMeta {
        if let FileType::File(meta) = &self.file_type {
            meta
        } else {
            panic!()
        }
    }

    pub fn get_file_meta_mut(&mut self) -> &mut FileMeta {
        if let FileType::File(meta) = &mut self.file_type {
            meta
        } else {
            panic!()
        }
    }

    pub fn get_dir_meta(&self) -> &DirMeta {
        if let FileType::Dir(meta) = &self.file_type {
            meta
        } else {
            panic!()
        }
    }

    pub fn get_dir_meta_mut(&mut self) -> &mut DirMeta {
        if let FileType::Dir(meta) = &mut self.file_type {
            meta
        } else {
            panic!()
        }
    }

    pub fn path(&self) -> &Path {
        self.common.path.as_ref().unwrap()
    }

    fn parent(&self) -> Rc<RefCell<Inode>> {
        self.get_dir_meta()
            .parent
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap()
    }

    fn split_borrow(&self) -> (&InodeCommon, &FileType) {
        (&self.common, &self.file_type)
    }

    fn split_borrow_mut(&mut self) -> (&mut InodeCommon, &mut FileType) {
        (&mut self.common, &mut self.file_type)
    }

    fn set_parent(&mut self, parent: Weak<RefCell<Inode>>) {
        self.get_dir_meta_mut().parent = Some(parent)
    }

    fn set_size(&mut self, size: u32) {
        self.common.size = size
    }

    fn inc_nlink(&mut self) {
        self.common.nlink += 1
    }

    fn add_dentry(&mut self, dentry: Dentry) {
        self.get_dir_meta_mut().dentries.push(dentry)
    }

    fn push_extent(&mut self, off: u32, len: u32, frag_off: u32) -> Option<()> {
        let (common, file_type) = self.split_borrow_mut();
        let FileType::File(file) = file_type else {
            panic!()
        };
        let codexfs_extent = CodexFsExtent { off, frag_off };
        log::info!("push extent {codexfs_extent:?}");
        file.extents.push(codexfs_extent);
        match (off + len).cmp(&common.size) {
            Ordering::Less => Some(()),
            Ordering::Equal => None,
            Ordering::Greater => panic!(),
        }
    }
}

impl Dentry {
    fn new(path: &Path, node: Rc<RefCell<Inode>>) -> Self {
        let metadata = path.symlink_metadata().unwrap();
        Dentry {
            path: Some(path.into()),
            file_name: path.file_name().unwrap().to_string_lossy().to_string(),
            file_type: metadata.file_type().into(),
            inode: node,
        }
    }

    pub fn file_name(&self) -> &str {
        &self.file_name
    }
}

impl From<&Ref<'_, Inode>> for CodexFsInode {
    fn from(inode: &Ref<'_, Inode>) -> Self {
        let blk_id = match &inode.file_type {
            FileType::File(file) => file.blk_id.unwrap_or(0),
            _ => 0,
        };
        let blks = match &inode.file_type {
            FileType::File(file) => file.extents.len() as _,
            _ => 0,
        };
        Self {
            format: CodexFsInodeFormat::CODEXFS_INODE_FLAT_PLAIN,
            mode: inode.common.mode,
            nlink: inode.common.nlink,
            size: inode.common.size,
            blk_id,
            ino: inode.common.nid,
            uid: inode.common.uid,
            gid: inode.common.gid,
            blks,
            reserved: [0; _],
        }
    }
}

impl From<&Dentry> for CodexFsDirent {
    fn from(dentry: &Dentry) -> Self {
        Self {
            nid: dentry.inode.borrow().common.nid,
            nameoff: 0,
            file_type: dentry.file_type,
            reserved: 0,
        }
    }
}

fn mkfs_load_inode_dir(path: &Path) -> Result<Rc<RefCell<Inode>>> {
    assert!(path.is_dir());

    let dir = Rc::new(RefCell::new(Inode::new(path)));

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();

        let child = mkfs_load_inode(&entry_path, Some(Rc::downgrade(&dir)))?;
        let child_dentry = Dentry::new(&entry_path, child);

        if child_dentry.file_type.is_dir() {
            dir.borrow_mut().inc_nlink();
        }
        dir.borrow_mut().add_dentry(child_dentry);
    }

    Ok(dir)
}

pub fn mkfs_load_inode(
    path: &Path,
    parent: Option<Weak<RefCell<Inode>>>,
) -> Result<Rc<RefCell<Inode>>> {
    let metadata = path.symlink_metadata()?;
    let ino = metadata.ino();

    let file_type = metadata.file_type().into();
    let inode = match file_type {
        CodexFsFileType::Unknown => panic!(),
        CodexFsFileType::File | CodexFsFileType::Symlink => {
            let inode = get_inode(ino).cloned().unwrap_or_else(|| {
                let child = Inode::new(path);
                Rc::new(RefCell::new(child))
            });
            inode.borrow_mut().inc_nlink();
            inode
        }
        CodexFsFileType::Dir => {
            let inode = mkfs_load_inode_dir(path)?;
            let parent = parent.unwrap_or_else(|| Rc::downgrade(&inode));
            inode.borrow_mut().set_parent(parent);
            let ndir = inode.borrow().get_dir_meta().dentries.len() + 2;
            let mut namesize = 1 + 2;
            for d in inode.borrow().get_dir_meta().dentries.iter() {
                namesize += d.file_name().len();
            }
            inode
                .borrow_mut()
                .set_size((ndir * size_of::<CodexFsDirent>() + namesize) as _);
            inode
        }
        CodexFsFileType::CharDevice => todo!(),
        CodexFsFileType::BlockDevice => todo!(),
        CodexFsFileType::Fifo => todo!(),
        CodexFsFileType::Socket => todo!(),
    };

    if get_inode(ino).is_none() {
        if inode.borrow().file_type.is_file() {
            get_cmpr_mgr_mut().push_file(inode.clone())?;
        }
        get_inode_vec_mut().inodes.push(inode.clone());
        insert_inode(ino, inode.clone());
    }

    Ok(inode)
}

pub fn mkfs_balloc_inode() {
    let buf_mgr = get_bufmgr_mut();

    for inode in get_inode_vec_mut().inodes.iter() {
        let mut guard = inode.borrow_mut();
        let (common, file_type) = guard.deref_mut().split_borrow_mut();
        match &file_type {
            FileType::File(file) => {
                let pos = buf_mgr.balloc(
                    (size_of::<CodexFsInode>() + file.extents.len() * size_of::<CodexFsExtent>())
                        as _,
                    BufferType::Inode,
                );
                common.nid = codexfs_nid(pos);
            }
            FileType::Dir { .. } => {
                let pos = buf_mgr.balloc(
                    size_of::<CodexFsInode>() as u64 + common.size as u64,
                    BufferType::Inode,
                );
                common.nid = codexfs_nid(pos);
            }
            FileType::CharDevice => todo!(),
            FileType::BlockDevice => todo!(),
            FileType::Fifo => todo!(),
            FileType::Socket => todo!(),
            FileType::Symlink => {
                let pos = buf_mgr.balloc(
                    size_of::<CodexFsInode>() as u64 + common.size as u64,
                    BufferType::Inode,
                );
                common.nid = codexfs_nid(pos);
            }
        }
    }
}

fn mkfs_dump_codexfs_inode(inode: &Rc<RefCell<Inode>>) -> Result<()> {
    log::info!(
        "path: {}, nid: {}",
        inode.borrow().path().display(),
        inode.borrow().common.nid
    );
    let inode_ref = inode.borrow();
    let codexfs_inode = CodexFsInode::from(&inode_ref);
    get_sb().img_file.write_all_at(
        bytes_of(&codexfs_inode),
        inode_ref.common.nid << CODEXFS_ISLOT_BITS,
    )?;
    Ok(())
}

pub fn mkfs_dump_inode_file_data() -> Result<()> {
    get_cmpr_mgr_mut().off = 0;

    let mut output = [0; CODEXFS_BLKSIZ as usize];
    let mut it = get_cmpr_mgr().files.iter();
    let (off, inode) = it.next().unwrap();
    let mut off = off;
    let mut inode = inode;

    while (get_cmpr_mgr().off as usize) < get_cmpr_mgr().origin_data.len() {
        let mut stream = Stream::new_microlzma_encoder(
            &LzmaOptions::new_preset(get_cmpr_mgr().lzma_level).unwrap(),
        )?;
        // let mut stream =
        //     Stream::new_easy_encoder(get_cmpr_mgr().lzma_level,
        // xz2::stream::Check::None).unwrap();
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
                .get_file_meta_mut()
                .blk_id
                .get_or_insert((woff >> CODEXFS_BLKSIZ_BITS) as _);
            log::info!(
                "path {}, blk_id {:?}",
                inode.borrow().path().display(),
                inode.borrow().get_file_meta().blk_id
            );
            let len = min(
                stream.total_in() - frag_off,
                *off + inode.borrow().common.size as u64 - get_cmpr_mgr().off,
            );
            if inode
                .borrow_mut()
                .push_extent((get_cmpr_mgr().off - *off) as _, len as _, frag_off as _)
                .is_none()
            {
                let Some((_off, _inode)) = it.next() else {
                    get_cmpr_mgr_mut().off += len;
                    break;
                };
                off = _off;
                inode = _inode;
            };
            get_cmpr_mgr_mut().off += len;
            frag_off += len;
        }

        get_sb_mut().end_data_blk_id = (woff >> CODEXFS_BLKSIZ_BITS) as _;
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
        let (common, file_type) = guard.deref().split_borrow();
        match file_type {
            FileType::File(file) => {
                let mut extents_off = (common.nid + 1) << CODEXFS_ISLOT_BITS;
                for codexfs_extent in file.extents.iter() {
                    sb.img_file
                        .write_all_at(bytes_of(codexfs_extent), extents_off)?;
                    extents_off += size_of::<CodexFsExtent>() as u64;
                }
                mkfs_dump_codexfs_inode(inode)?;
            }
            FileType::Dir(dir) => {
                let mut dirents = Vec::new();
                let mut names = Vec::new();
                let mut nameoff = (size_of::<CodexFsDirent>() * (dir.dentries.len() + 2)) as u16;

                let dot_dirent = CodexFsDirent {
                    nid: common.nid,
                    nameoff,
                    file_type: file_type.into(),
                    reserved: 0,
                };
                dirents.push(dot_dirent);
                names.push(".");
                nameoff += 1;

                let dotdot_dirent = CodexFsDirent {
                    nid: guard.parent().borrow().common.nid,
                    nameoff,
                    file_type: file_type.into(),
                    reserved: 0,
                };
                dirents.push(dotdot_dirent);
                names.push("..");
                nameoff += 2;

                for dentry in dir.dentries.iter() {
                    let mut codexfs_dirent = CodexFsDirent::from(dentry);
                    codexfs_dirent.nameoff = nameoff;
                    dirents.push(codexfs_dirent);
                    names.push(dentry.file_name());
                    nameoff += u16::try_from(dentry.file_name().len())?;
                }

                let mut dirent_off = (common.nid + 1) << CODEXFS_ISLOT_BITS;
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
                    ((common.nid + 1) << CODEXFS_ISLOT_BITS) + common.size as u64,
                    name_off
                );

                mkfs_dump_codexfs_inode(inode)?;
            }
            FileType::CharDevice => todo!(),
            FileType::BlockDevice => todo!(),
            FileType::Fifo => todo!(),
            FileType::Socket => todo!(),
            FileType::Symlink => {
                let link = fs::read_link(guard.path())?;
                sb.img_file.write_all_at(
                    link.to_string_lossy().as_bytes(),
                    (common.nid + 1) << CODEXFS_ISLOT_BITS,
                )?;
                mkfs_dump_codexfs_inode(inode)?;
            }
        }
    }

    Ok(())
}

pub fn fuse_load_inode_file(nid: u64, codexfs_inode: &CodexFsInode) -> Result<Rc<RefCell<Inode>>> {
    let mut inode = Inode::from_codexfs_inode(codexfs_inode, nid);
    let mut codexfs_extent_off = (nid + 1) << CODEXFS_ISLOT_BITS;
    let mut codexfs_extent_buf = [0; size_of::<CodexFsExtent>()];

    log::info!("nid {nid} blks {}", { codexfs_inode.blks });
    for _ in 0..codexfs_inode.blks {
        get_sb()
            .img_file
            .read_exact_at(&mut codexfs_extent_buf, codexfs_extent_off)?;
        let extent: CodexFsExtent = *from_bytes::<CodexFsExtent>(&codexfs_extent_buf);
        log::info!("nid {nid} push extent");
        inode.get_file_meta_mut().extents.push(extent);
        codexfs_extent_off += size_of::<CodexFsExtent>() as u64;
    }

    Ok(Rc::new(RefCell::new(inode)))
}

pub fn fuse_load_inode_dir(nid: u64, codexfs_inode: &CodexFsInode) -> Result<Rc<RefCell<Inode>>> {
    let mut inode = Inode::from_codexfs_inode(codexfs_inode, nid);
    let dirents_start = (nid + 1) << CODEXFS_ISLOT_BITS;
    let mut dirent_buf = [0; size_of::<CodexFsDirent>()];
    get_sb()
        .img_file
        .read_exact_at(&mut dirent_buf, dirents_start)?;
    let codexfs_dirent: CodexFsDirent = *from_bytes(&dirent_buf);

    let ndir = codexfs_dirent.nameoff / (size_of::<CodexFsDirent>() as u16);
    let mut dirents = Vec::new();
    let mut dirent_buf = [0; size_of::<CodexFsDirent>()];
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
                inode.common.size as _
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
        let child_dentry = Dentry {
            path: None,
            file_name,
            file_type: dirents[i as usize].file_type,
            inode: child_inode,
        };
        inode.add_dentry(child_dentry);
    }
    Ok(Rc::new(RefCell::new(inode)))
}

pub fn fuse_load_inode(nid: u64) -> Result<Rc<RefCell<Inode>>> {
    let mut inode_buf = [0; size_of::<CodexFsInode>()];
    let img_file = &get_sb().img_file;

    log::info!("load inode nid {nid}");
    img_file.read_exact_at(&mut inode_buf, nid << CODEXFS_ISLOT_BITS)?;
    let codexfs_inode: &CodexFsInode = from_bytes(&inode_buf);

    let file_type: FileType = codexfs_inode.into();
    if !file_type.is_dir() {
        if let Some(inode) = get_inode(codexfs_inode.ino) {
            return Ok(inode.clone());
        }
    }
    let inode = match file_type {
        FileType::File { .. } => fuse_load_inode_file(nid, codexfs_inode)?,
        FileType::Dir { .. } => fuse_load_inode_dir(nid, codexfs_inode)?,
        FileType::CharDevice => todo!(),
        FileType::BlockDevice => todo!(),
        FileType::Fifo => todo!(),
        FileType::Socket => todo!(),
        FileType::Symlink => {
            let inode = Inode::from_codexfs_inode(codexfs_inode, nid);
            Rc::new(RefCell::new(inode))
        }
    };

    insert_inode(inode.borrow().common.ino, inode.clone());
    Ok(inode)
}

pub fn fuse_read_inode_file(inode: Rc<RefCell<Inode>>, off: u32, len: u32) -> Result<Vec<u8>> {
    assert!(inode.borrow().file_type.is_file());

    const MEM_LIMIT: usize = 16 * 1024 * 1024;

    let guard = inode.borrow();
    let (common, file_type) = guard.split_borrow();
    let idx = (off >> CODEXFS_BLKSIZ_BITS) as usize;
    let FileType::File(file) = file_type else {
        panic!();
    };
    log::info!("off: {}, len {}", off, len);
    let len = min(len, common.size - off);
    let mut len_left = len;
    let mut buf = Vec::new();

    log::info!("off: {}, len {}", off, len);

    let mut input = [0; CODEXFS_BLKSIZ as usize];

    log::info!("extents: {:?}", file.extents);
    let i = file.extents.partition_point(|&e| e.off <= off);
    log::info!("read_inode_file {}", i);
    for (i, e) in file.extents.iter().enumerate().skip(i - 1) {
        log::info!("i {i}, e {:?}", e);

        let mut output = Vec::with_capacity(MEM_LIMIT);
        let blk_id = file.blk_id.unwrap() + i as u32;
        get_sb()
            .img_file
            .read_exact_at(&mut input, (blk_id as u64) << (CODEXFS_BLKSIZ_BITS as u64))?;

        log::info!(
            "end blk id {}, end blk sz {}",
            get_sb().end_data_blk_id,
            get_sb().end_data_blk_sz
        );
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
        fs::{self, File},
        path::Path,
        rc::Rc,
    };

    use anyhow::{Ok, Result};

    use crate::{
        compress::set_cmpr_mgr,
        inode::{get_inode_by_path, mkfs_load_inode},
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

            assert!(Rc::ptr_eq(&root_inode.borrow().parent(), &root_inode));
            assert!(Rc::ptr_eq(hello_inode, hardlink_inode));

            assert_eq!(root_inode.borrow().common.nlink, 3);
            assert_eq!(subdir_inode.borrow().common.nlink, 2);
            assert_eq!(hello_inode.borrow().common.nlink, 2);
        }

        fs::remove_dir_all(root)?;
        fs::remove_file(img_path)?;

        Ok(())
    }
}
