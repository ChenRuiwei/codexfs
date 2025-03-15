use std::{cell::OnceCell, rc::Rc};

use anyhow::{Ok, Result};

use crate::{
    inode::{File, Inode},
    off_t,
};

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
    pub off: off_t,
    pub files: Vec<(off_t, Rc<Inode<File>>)>,
    pub lzma_level: u32,
}

impl CompressManager {
    pub fn new(lzma_level: u32) -> Self {
        Self {
            lzma_level,
            ..Default::default()
        }
    }

    pub fn push_file(&mut self, inode: Rc<Inode<File>>) -> Result<()> {
        let content = inode.read_to_end()?;
        self.origin_data.extend(content);
        self.files.push((self.off, inode.clone()));
        self.off += inode.inner.size as u64;
        log::info!("push file {}", inode.meta.path.as_ref().unwrap().display());
        Ok(())
    }
}
