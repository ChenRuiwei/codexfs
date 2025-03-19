use std::{
    any::Any,
    cell::{Ref, RefCell},
    cmp::Ordering,
    io::Read,
    os::unix::fs::MetadataExt,
    path::Path,
    rc::Rc,
};

use anyhow::{Ok, Result};
use bytemuck::from_bytes;
use tlsh_fixed::Tlsh;

use super::{Inode, InodeFactory, InodeMeta, InodeOps};
use crate::{
    CodexFsExtent, CodexFsFileType, CodexFsInode, blk_off_t, blk_size_t, blk_t,
    compress::calc_tlsh,
    inode::InodeMetaInner,
    nid_to_inode_meta_off,
    sb::{get_sb, get_sb_mut},
    size_t,
};

#[derive(Debug, Default)]
pub struct File {
    pub size: size_t,
    pub inner: RefCell<FileInner>,
}

#[derive(Debug, Default)]
pub struct FileInner {
    pub blk_id: Option<blk_t>,
    pub blk_off: Option<blk_off_t>,
    pub extents: Vec<CodexFsExtent>,
    pub content: Option<Vec<u8>>,
    pub tlsh: Option<Tlsh>,
}

impl InodeFactory for Inode<File> {
    fn from_path(path: &Path) -> Self {
        let metadata = path.symlink_metadata().unwrap();
        log::info!("{}, size {}", path.display(), metadata.len());
        let mut file = std::fs::File::open(path).unwrap();
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        let tlsh = calc_tlsh(&content);
        Self {
            meta: InodeMeta {
                path: Some(path.into()),
                ino: get_sb_mut().get_ino_and_inc(),
                gid: metadata.gid() as _,
                uid: metadata.uid() as _,
                mode: metadata.mode() as _,
                inner: RefCell::new(InodeMetaInner {
                    nlink: 0,
                    nid: 0,
                    meta_size: None,
                }),
            },
            itype: File {
                size: metadata.len() as _,
                inner: RefCell::new(FileInner {
                    content: Some(content),
                    tlsh,
                    ..Default::default()
                }),
            },
        }
    }

    fn from_codexfs_inode(codexfs_inode: &CodexFsInode, nid: u64) -> Self {
        Self {
            meta: InodeMeta {
                path: None,
                ino: codexfs_inode.ino,
                uid: codexfs_inode.uid,
                gid: codexfs_inode.gid,
                mode: codexfs_inode.mode,
                inner: RefCell::new(InodeMetaInner {
                    nid,
                    meta_size: None,
                    nlink: codexfs_inode.nlink,
                }),
            },
            itype: File {
                size: codexfs_inode.size,
                inner: RefCell::new(FileInner {
                    blk_id: Some(codexfs_inode.blk_id),
                    blk_off: if !get_sb().compress {
                        Some(unsafe { codexfs_inode.u.blk_off })
                    } else {
                        None
                    },
                    ..Default::default()
                }),
            },
        }
    }

    fn fuse_load(codexfs_inode: &CodexFsInode, nid: u64) -> Result<Rc<Self>> {
        let inode = Self::from_codexfs_inode(codexfs_inode, nid);
        let extents_off = nid_to_inode_meta_off(nid);
        let mut extent_buf = [0; size_of::<CodexFsExtent>()];

        if get_sb().compress {
            let blks = unsafe { codexfs_inode.u.blks };
            log::info!("nid {nid} blks {}", blks);
            for i in 0..blks {
                get_sb().read_exact_at(
                    &mut extent_buf,
                    extents_off + (i as usize * size_of::<CodexFsExtent>()) as u64,
                )?;
                let extent: CodexFsExtent = *from_bytes::<CodexFsExtent>(&extent_buf);
                log::info!("nid {nid} push extent");
                inode.itype.inner.borrow_mut().extents.push(extent);
            }
        }

        Ok(Rc::new(inode))
    }
}

impl InodeOps for Inode<File> {
    fn meta(&self) -> &InodeMeta {
        &self.meta
    }

    fn file_type(&self) -> CodexFsFileType {
        CodexFsFileType::File
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Inode<File> {
    pub(crate) fn push_extent(&self, off: u32, len: u32, frag_off: u32) -> Option<()> {
        let codexfs_extent = CodexFsExtent { off, frag_off };
        log::info!("push extent {codexfs_extent:?}");
        self.itype.inner.borrow_mut().extents.push(codexfs_extent);
        match (off + len).cmp(&self.itype.size) {
            Ordering::Less => Some(()),
            Ordering::Equal => None,
            Ordering::Greater => panic!(),
        }
    }
}
