use std::{
    any::Any,
    cell::RefCell,
    os::unix::fs::{FileExt, MetadataExt},
    path::Path,
    rc::{Rc, Weak},
};

use anyhow::Result;
use bytemuck::from_bytes;

use super::{Dentry, Inode, InodeFactory, InodeOps, insert_inode};
use crate::{
    CodexFsFileType, CodexFsInode,
    inode::{InodeMeta, InodeMetaInner},
    nid_to_inode_off,
    sb::{get_sb, get_sb_mut},
};

#[derive(Debug, Default)]
pub struct Dir {
    pub inner: RefCell<DirInner>,
}

#[derive(Debug, Default)]
pub struct DirInner {
    pub parent: Option<Weak<Inode<Dir>>>, // root points to itself
    pub dentries: Vec<Dentry>,            // child dentries
}

impl InodeFactory for Inode<Dir> {
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
                    nlink: 2,
                    nid: 0,
                    meta_size: None,
                }),
            },
            inner: Dir::default(),
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
                    nlink: codexfs_inode.nlink,
                    nid,
                    meta_size: Some(codexfs_inode.size),
                }),
            },
            inner: Dir {
                ..Default::default()
            },
        }
    }
}

impl InodeOps for Inode<Dir> {
    fn meta(&self) -> &InodeMeta {
        &self.meta
    }

    fn file_type(&self) -> CodexFsFileType {
        CodexFsFileType::Dir
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Inode<Dir> {
    pub fn load_from_nid(nid: u64) -> Result<Rc<Self>> {
        let mut inode_buf = [0; size_of::<CodexFsInode>()];
        get_sb()
            .img_file
            .read_exact_at(&mut inode_buf, nid_to_inode_off(nid))?;
        let codexfs_inode: &CodexFsInode = from_bytes(&inode_buf);
        let inode = Rc::new(Self::from_codexfs_inode(codexfs_inode, nid));
        insert_inode(inode.meta.ino, inode.clone());
        Ok(inode)
    }

    pub(crate) fn parent(&self) -> Rc<Inode<Dir>> {
        self.inner
            .inner
            .borrow()
            .parent
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap()
    }

    pub(crate) fn set_parent(&self, parent: Weak<Inode<Dir>>) {
        self.inner.inner.borrow_mut().parent = Some(parent)
    }

    pub(crate) fn add_dentry(&self, dentry: Dentry) {
        self.inner.inner.borrow_mut().dentries.push(dentry)
    }
}
