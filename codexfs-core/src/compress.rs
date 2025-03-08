use std::{
    cell::{OnceCell, RefCell},
    fs::File,
    io::Read,
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
        let mut file = File::open(inode.borrow().path())?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        self.origin_data.extend(content);
        self.files.push((self.off, inode.clone()));
        self.off += inode.borrow().common.size as u64;

        log::info!("push file {}", inode.borrow().path().display());
        Ok(())
    }
}
