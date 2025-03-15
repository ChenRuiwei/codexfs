use std::{any::Any, cell::RefCell, os::unix::fs::MetadataExt, path::Path};

use super::{Inode, InodeFactory, InodeMeta, InodeOps};
use crate::{CodexFsFileType, CodexFsInode, inode::InodeMetaInner, sb::get_sb_mut};

#[derive(Debug, Default)]
pub struct SymLink {}

impl InodeFactory for Inode<SymLink> {
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
                    meta_size: Some(metadata.len() as _),
                }),
            },
            itype: SymLink::default(),
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
                    nlink: codexfs_inode.nlink,
                    meta_size: Some(codexfs_inode.size),
                }),
            },
            itype: SymLink::default(),
        }
    }
}

impl InodeOps for Inode<SymLink> {
    fn meta(&self) -> &InodeMeta {
        &self.meta
    }

    fn file_type(&self) -> CodexFsFileType {
        CodexFsFileType::Symlink
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
