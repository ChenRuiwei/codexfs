use std::{any::Any, cmp::Ordering, io::Read, os::unix::fs::MetadataExt, path::Path};

use anyhow::Result;

use super::{Inode, InodeFactory, InodeMeta, InodeOps};
use crate::{CodexFsExtent, CodexFsFileType, CodexFsInode, blk_t, sb::get_sb_mut, size_t};

#[derive(Debug, Default)]
pub struct File {
    pub size: size_t,
    pub blk_id: Option<blk_t>,
    pub extents: Vec<CodexFsExtent>,
}

impl InodeFactory for Inode<File> {
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
                meta_size: None,
            },
            inner: File {
                size: metadata.len() as _,
                ..Default::default()
            },
        }
    }

    fn from_codexfs_inode(codexfs_inode: &CodexFsInode, nid: u64) -> Self {
        Self {
            meta: InodeMeta {
                path: None,
                meta_size: None,
                ino: codexfs_inode.ino,
                uid: codexfs_inode.uid,
                gid: codexfs_inode.gid,
                mode: codexfs_inode.mode,
                nid,
                nlink: codexfs_inode.nlink,
            },
            inner: File {
                size: codexfs_inode.size,
                blk_id: Some(codexfs_inode.blk_id),
                ..Default::default()
            },
        }
    }
}

impl InodeOps for Inode<File> {
    fn meta(&self) -> &InodeMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut InodeMeta {
        &mut self.meta
    }

    fn file_type(&self) -> CodexFsFileType {
        CodexFsFileType::File
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Inode<File> {
    pub fn read_to_end(&self) -> Result<Vec<u8>> {
        let mut file = std::fs::File::open(self.meta.path())?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        Ok(content)
    }

    pub(crate) fn push_extent(&mut self, off: u32, len: u32, frag_off: u32) -> Option<()> {
        let codexfs_extent = CodexFsExtent { off, frag_off };
        log::info!("push extent {codexfs_extent:?}");
        self.inner.extents.push(codexfs_extent);
        match (off + len).cmp(&self.inner.size) {
            Ordering::Less => Some(()),
            Ordering::Equal => None,
            Ordering::Greater => panic!(),
        }
    }
}
