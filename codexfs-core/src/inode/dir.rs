use std::{
    any::Any,
    cell::RefCell,
    os::unix::fs::MetadataExt,
    path::Path,
    rc::{Rc, Weak},
};

use anyhow::Result;
use bytemuck::from_bytes;

use super::{Dentry, Inode, InodeFactory, InodeOps, insert_inode};
use crate::{
    CodexFsDirent, CodexFsFileType, CodexFsInode,
    inode::{InodeMeta, InodeMetaInner, fuse_load_inode},
    nid_to_inode_meta_off, nid_to_inode_off,
    sb::{get_sb, get_sb_mut},
    utils::is_dot_or_dotdot,
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
            itype: Dir::default(),
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
            itype: Dir {
                ..Default::default()
            },
        }
    }

    fn fuse_load(codexfs_inode: &CodexFsInode, nid: u64) -> Result<Rc<Self>> {
        let inode = Rc::new(Inode::<Dir>::from_codexfs_inode(codexfs_inode, nid));
        let dirents_off = nid_to_inode_meta_off(nid);
        let mut dirent_buf = [0; size_of::<CodexFsDirent>()];
        let ndir = {
            get_sb().read_exact_at(&mut dirent_buf, dirents_off)?;
            let codexfs_dirent: CodexFsDirent = *from_bytes(&dirent_buf);
            codexfs_dirent.nameoff / (size_of::<CodexFsDirent>() as u16)
        };

        let mut dirents = Vec::new();
        for i in 0..ndir {
            get_sb().read_exact_at(
                &mut dirent_buf,
                dirents_off + (i as usize * size_of::<CodexFsDirent>()) as u64,
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
                get_sb().read_exact_at(&mut name_buf, dirents_off + startoff as u64)?;
                String::from_utf8(name_buf)?
            };
            log::debug!("{}", file_name);
            if is_dot_or_dotdot(&file_name) {
                continue;
            }
            let child_inode = fuse_load_inode(dirents[i as usize].nid)?;
            assert_eq!(dirents[i as usize].file_type, child_inode.file_type());
            if let Some(child_dir) = child_inode.downcast_dir_ref() {
                child_dir.set_parent(Rc::downgrade(&inode));
            }
            let child_dentry = Dentry::new_name(file_name, child_inode);
            inode.add_dentry(child_dentry);
        }

        Ok(inode)
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
        get_sb().read_exact_at(&mut inode_buf, nid_to_inode_off(nid))?;
        let codexfs_inode: &CodexFsInode = from_bytes(&inode_buf);
        let inode = Rc::new(Self::from_codexfs_inode(codexfs_inode, nid));
        insert_inode(inode.meta.ino, inode.clone());
        Ok(inode)
    }

    pub(crate) fn parent(&self) -> Rc<Inode<Dir>> {
        self.itype
            .inner
            .borrow()
            .parent
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap()
    }

    pub(crate) fn set_parent(&self, parent: Weak<Inode<Dir>>) {
        self.itype.inner.borrow_mut().parent = Some(parent)
    }

    pub(crate) fn add_dentry(&self, dentry: Dentry) {
        self.itype.inner.borrow_mut().dentries.push(dentry)
    }
}
