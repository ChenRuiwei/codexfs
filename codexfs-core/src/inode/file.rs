use std::{
    any::Any,
    cell::{Ref, RefCell},
    cmp::Ordering,
    io::Read,
    os::unix::fs::MetadataExt,
    path::Path,
};

use anyhow::{Ok, Result};
use tlsh_fixed::Tlsh;

use super::{Inode, InodeFactory, InodeMeta, InodeOps};
use crate::{
    CodexFsExtent, CodexFsFileType, CodexFsInode, blk_t, compress::get_tlsh, inode::InodeMetaInner,
    sb::get_sb_mut, size_t,
};

#[derive(Debug, Default)]
pub struct File {
    pub size: size_t,
    pub inner: RefCell<FileInner>,
}

#[derive(Debug, Default)]
pub struct FileInner {
    pub blk_id: Option<blk_t>,
    pub extents: Vec<CodexFsExtent>,
    pub content: Option<Vec<u8>>,
    pub tlsh: Option<Tlsh>,
}

impl InodeFactory for Inode<File> {
    fn from_path(path: &Path) -> Self {
        let metadata = path.symlink_metadata().unwrap();
        log::info!("{}, size {}", path.display(), metadata.len());
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
                ..Default::default()
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
                    ..Default::default()
                }),
            },
        }
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
    pub fn read_to_end(&self) -> Result<Ref<[u8]>> {
        {
            let mut guard = self.itype.inner.borrow_mut();
            if guard.content.is_none() {
                let mut file = std::fs::File::open(self.meta.path()).unwrap();
                let mut content = Vec::new();
                file.read_to_end(&mut content).unwrap();
                guard.tlsh = get_tlsh(&content);
                guard.content = Some(content);
            }
        }
        {
            let guard = self.itype.inner.borrow();
            Ok(Ref::map(guard, |inner| {
                inner.content.as_ref().unwrap().as_slice()
            }))
        }
    }

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
