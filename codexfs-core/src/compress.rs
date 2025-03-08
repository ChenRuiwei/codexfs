use std::{
    cell::{OnceCell, RefCell},
    fs::File,
    io::Read,
    rc::Rc,
};

use anyhow::{Ok, Result};

use crate::inode::Inode;

pub const LZMA_LEVEL: u32 = 6;

#[derive(Default)]
pub struct CompressManager {
    pub origin_data: Vec<u8>,
    pub off: u64,
    pub files: Vec<(u64, Rc<RefCell<Inode>>)>,
}

static mut COMPRESS_MANAGER: OnceCell<CompressManager> = OnceCell::new();

pub fn get_cmpr_mgr() -> &'static CompressManager {
    unsafe { COMPRESS_MANAGER.get_or_init(CompressManager::default) }
}

pub fn get_cmpr_mgr_mut() -> &'static mut CompressManager {
    unsafe { COMPRESS_MANAGER.get_mut_or_init(CompressManager::default) }
}

impl CompressManager {
    pub fn push_file(&mut self, inode: Rc<RefCell<Inode>>) -> Result<()> {
        assert!(inode.borrow().file_type.is_file());
        let mut file = File::open(inode.borrow().path())?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        self.origin_data.extend(content);
        self.files.push((self.off, inode.clone()));
        self.off += inode.borrow().common.size;
        Ok(())
    }
}
