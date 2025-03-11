use std::{
    cell::{OnceCell, RefCell},
    rc::Rc,
};

use anyhow::{Ok, Result};

use crate::inode::Inode;

static mut COMPRESS_MANAGER: OnceCell<CompressManager> = OnceCell::new();

pub fn set_cmpr_mgr(lzma_level: u32) {
    unsafe {
        COMPRESS_MANAGER
            .set(CompressManager::new(lzma_level))
            .unwrap()
    }
}

pub fn get_cmpr_mgr() -> &'static CompressManager {
    unsafe { COMPRESS_MANAGER.get().unwrap() }
}

pub fn get_cmpr_mgr_mut() -> &'static mut CompressManager {
    unsafe { COMPRESS_MANAGER.get_mut().unwrap() }
}

#[derive(Default, Debug)]
pub struct CompressManager {
    pub origin_data: Vec<u8>,
    pub off: u64,
    pub files: Vec<(u64, Rc<RefCell<Inode>>)>,
    pub lzma_level: u32,
}

impl CompressManager {
    pub fn new(lzma_level: u32) -> Self {
        Self {
            lzma_level,
            ..Default::default()
        }
    }

    pub fn push_file(&mut self, inode: Rc<RefCell<Inode>>) -> Result<()> {
        assert!(inode.borrow().file_type.is_file());
        let content = inode.borrow().read_to_end()?;
        self.origin_data.extend(content);
        self.files.push((self.off, inode.clone()));
        self.off += inode.borrow().get_file_meta().size as u64;
        log::info!("push file {}", inode.borrow().path().display());
        Ok(())
    }
}
