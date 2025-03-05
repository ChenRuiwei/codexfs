use std::{
    array,
    cell::{OnceCell, RefCell},
    cmp::{self, Ordering},
    ops::{Deref, DerefMut},
    rc::Rc,
};

use crate::{CODEXFS_BLKSIZ, CODEXFS_BLKSIZ_BITS, CodexFsInode, utils::round_up};

pub enum BufferType {
    Meta,
    Inode,
    Data,
}

pub fn get_alignment(btype: BufferType) -> u16 {
    match btype {
        BufferType::Meta => 1,
        BufferType::Inode => size_of::<CodexFsInode>() as _,
        BufferType::Data => 1,
    }
}

pub fn get_bufmgr_mut() -> &'static mut BufferManager {
    static mut BUFFER_MANAGER: OnceCell<BufferManager> = OnceCell::new();
    unsafe { BUFFER_MANAGER.get_mut_or_init(BufferManager::new) }
}

pub struct BufferBlockTable(
    [Vec<Rc<RefCell<BufferBlock>>>; (CODEXFS_BLKSIZ + 1) as _], // index means unused size
);

impl Deref for BufferBlockTable {
    type Target = [Vec<Rc<RefCell<BufferBlock>>>; (CODEXFS_BLKSIZ + 1) as _];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BufferBlockTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl BufferBlockTable {
    fn new() -> Self {
        Self(array::from_fn(|_| Vec::new()))
    }
}

pub struct BufferManager {
    pub table: BufferBlockTable,
    pub tail_blk: Rc<RefCell<BufferBlock>>,
}

impl BufferManager {
    fn new() -> Self {
        let buf_blk = Rc::new(RefCell::new(BufferBlock::new(0)));
        let mut buf_mgr = Self {
            table: BufferBlockTable::new(),
            tail_blk: buf_blk.clone(),
        };
        buf_mgr.push_block(buf_blk);
        buf_mgr
    }

    pub fn balloc(&mut self, size: u64, btype: BufferType) -> u64 {
        let alignment = get_alignment(btype);
        assert!(alignment <= CODEXFS_BLKSIZ);
        let aligned_size = round_up(size, alignment as _);

        if let Some(pos) = self.bfind(aligned_size, alignment) {
            return pos;
        }

        self.balloc_contig(aligned_size, alignment)
    }

    fn bfind(&mut self, aligned_size: u64, alignment: u16) -> Option<u64> {
        assert_eq!(aligned_size, round_up(aligned_size, alignment as _));
        if aligned_size > CODEXFS_BLKSIZ as _ {
            return None;
        }
        let size = aligned_size as u16;
        for i in size..CODEXFS_BLKSIZ + 1 {
            let i = i as usize;
            if self.table[i].is_empty() {
                continue;
            }
            let buf_blk = self.table[i].pop().unwrap();
            let pos = buf_blk.borrow().pos();
            buf_blk.borrow_mut().off += size;
            self.push_block(buf_blk);
            return Some(pos);
        }
        None
    }

    fn balloc_contig(&mut self, aligned_size: u64, alignment: u16) -> u64 {
        assert_eq!(aligned_size, round_up(aligned_size, alignment as _));
        let aligned_off = round_up(self.tail_blk.borrow().off, alignment);
        let (pos, mut size_left) = match aligned_off.cmp(&CODEXFS_BLKSIZ) {
            Ordering::Less => {
                assert!((aligned_off as u64 + aligned_size) > CODEXFS_BLKSIZ as u64);
                let pos = self.tail_blk.borrow().pos();
                let size_left = aligned_size - ((CODEXFS_BLKSIZ - aligned_off) as u64);
                (pos, size_left)
            }
            Ordering::Equal => {
                let pos = (self.tail_blk_id() + 1) << CODEXFS_BLKSIZ_BITS;
                let size_left = aligned_size;
                (pos, size_left)
            }
            Ordering::Greater => panic!(),
        };

        while size_left > 0 {
            let mut buf_blk = BufferBlock::new(self.tail_blk_id() + 1);
            buf_blk.off = cmp::min(CODEXFS_BLKSIZ as u64, size_left) as _;
            size_left -= buf_blk.off as u64;
            let buf_blk = Rc::new(RefCell::new(buf_blk));
            self.tail_blk = buf_blk.clone();
            self.push_block(buf_blk);
        }

        pos
    }

    pub fn tail_blk_id(&self) -> u64 {
        self.tail_blk.borrow().blk_id
    }

    fn push_block(&mut self, buf_blk: Rc<RefCell<BufferBlock>>) {
        let off = buf_blk.borrow().off;
        self.table[(CODEXFS_BLKSIZ - off) as usize].push(buf_blk);
    }
}

pub struct BufferBlock {
    pub blk_id: u64,
    pub off: u16,
}

impl BufferBlock {
    fn new(blk_id: u64) -> Self {
        Self { blk_id, off: 0 }
    }

    fn pos(&self) -> u64 {
        (self.blk_id << CODEXFS_BLKSIZ_BITS) + (self.off as u64)
    }
}
