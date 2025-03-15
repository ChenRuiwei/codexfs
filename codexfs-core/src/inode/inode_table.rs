use std::{cell::OnceCell, collections::HashMap, os::unix::fs::MetadataExt, path::Path};

use crate::{ino_t, inode::InodeHandle};

pub(crate) type InodeTable = HashMap<ino_t, InodeHandle>;

fn get_inode_table_mut() -> &'static mut InodeTable {
    static mut INODE_TABLE: OnceCell<InodeTable> = OnceCell::new();
    unsafe { INODE_TABLE.get_mut_or_init(HashMap::new) }
}

pub fn get_inode(ino: ino_t) -> Option<&'static InodeHandle> {
    get_inode_table_mut().get(&ino)
}

pub(crate) fn get_inode_by_path(path: &Path) -> Option<&'static InodeHandle> {
    let ino = path.symlink_metadata().unwrap().ino() as _;
    get_inode(ino)
}

pub(crate) fn insert_inode(ino: ino_t, inode: InodeHandle) {
    get_inode_table_mut().insert(ino, inode);
}

pub type InodeVec = Vec<InodeHandle>;

pub fn get_inode_vec_mut() -> &'static mut InodeVec {
    static mut INODE_VEC: OnceCell<InodeVec> = OnceCell::new();
    unsafe { INODE_VEC.get_mut_or_init(Vec::new) }
}
