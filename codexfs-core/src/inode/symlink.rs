use std::{any::Any, os::unix::fs::MetadataExt, path::Path};

use super::{Inode, InodeFactory, InodeMeta, InodeOps};
use crate::{CodexFsFileType, CodexFsInode, sb::get_sb_mut};

#[derive(Debug, Default)]
pub struct SymLink {}

impl InodeFactory for Inode<SymLink> {
    fn from_path(path: &Path) -> Self {
        let metadata = path.symlink_metadata().unwrap();
        log::info!("{}, size {}", path.display(), metadata.len());
        Self {
            meta: InodeMeta {
                path: Some(path.into()),
                nlink: 0,
                ino: get_sb_mut().get_ino_and_inc(),
                gid: metadata.gid() as _,
                uid: metadata.uid() as _,
                nid: 0,
                mode: metadata.mode() as _,
                meta_size: Some(metadata.len() as _),
            },
            inner: SymLink {
                ..Default::default()
            },
        }
    }

    fn from_codexfs_inode(codexfs_inode: &CodexFsInode, nid: u64) -> Self {
        Self {
            meta: InodeMeta {
                path: None,
                meta_size: Some(codexfs_inode.size),
                ino: codexfs_inode.ino,
                uid: codexfs_inode.uid,
                gid: codexfs_inode.gid,
                mode: codexfs_inode.mode,
                nid,
                nlink: codexfs_inode.nlink,
            },
            inner: SymLink {
                ..Default::default()
            },
        }
    }
}

impl InodeOps for Inode<SymLink> {
    fn meta(&self) -> &InodeMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut InodeMeta {
        &mut self.meta
    }

    fn file_type(&self) -> CodexFsFileType {
        CodexFsFileType::Symlink
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
