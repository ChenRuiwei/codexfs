use std::{
    cell::{OnceCell, Ref, RefCell},
    collections::HashMap,
    fs::{self, File, FileType},
    io::{self, Read},
    os::unix::fs::{FileExt, MetadataExt},
    path::Path,
    rc::Rc,
};

use crate::{
    CodexFsDirent, CodexFsFileType, CodexFsInode, CodexFsInodeFormat, codexfs_blknr,
    codexfs_blkoff, codexfs_nid,
    sb::{get_mut_sb, get_sb},
};

type InodeTable = HashMap<u64, Rc<RefCell<Inode>>>;

fn get_mut_inode_table() -> &'static mut InodeTable {
    static mut FILE_NODE_TABLE: OnceCell<InodeTable> = OnceCell::new();
    unsafe { FILE_NODE_TABLE.get_mut_or_init(HashMap::new) }
}

fn get_inode(ino: u64) -> Option<&'static Rc<RefCell<Inode>>> {
    get_mut_inode_table().get(&ino)
}

fn insert_inode(ino: u64, node: Rc<RefCell<Inode>>) {
    get_mut_inode_table().insert(ino, node);
}

#[derive(Debug)]
pub struct Inode {
    pub path: Box<Path>,
    pub file_type: FileType,
    pub size: u64,
    pub dentries: Vec<Dentry>, // TODO: handle dot and dotdot

    // Fields prefixed with "cf" (for codexfs) are unrelated to the original file system.
    pub cf_s_off: Option<u64>,
    pub cf_ino: u32,
    pub cf_nid: u64,
    pub cf_nlink: u16,
}

#[derive(Debug)]
pub struct Dentry {
    pub path: Box<Path>,
    pub file_name: String,
    pub file_type: FileType,
    pub inode: Rc<RefCell<Inode>>,
}

impl Inode {
    fn new(path: &Path) -> Self {
        let metadata = path.metadata().unwrap();
        Self {
            path: path.into(),
            file_type: metadata.file_type(),
            dentries: Vec::new(),
            cf_s_off: None,
            size: metadata.len(),
            cf_nlink: 0,
            cf_ino: get_mut_sb().get_ino_and_inc(),
            cf_nid: 0,
        }
    }

    fn new_dir(path: &Path) -> Self {
        let metadata = path.metadata().unwrap();
        Self {
            path: path.into(),
            file_type: metadata.file_type(),
            dentries: Vec::new(),
            cf_s_off: None,
            size: metadata.len(),
            cf_nlink: 2,
            cf_ino: get_mut_sb().get_ino_and_inc(),
            cf_nid: 0,
        }
    }

    fn inc_nlink(&mut self) {
        self.cf_nlink += 1
    }

    fn add_dentry(&mut self, dentry: Dentry) {
        self.dentries.push(dentry);
    }

    fn file_type(&self) -> FileType {
        self.file_type
    }

    pub fn print_recursive(&self, depth: usize) {
        let indent = "\t".repeat(depth);
        println!(
            "{}Inode: {:?}, {:?}, size={}, nlink={}",
            indent,
            self.path,
            CodexFsFileType::from(self.file_type),
            self.size,
            self.cf_nlink
        );

        for dentry in &self.dentries {
            dentry.inode.borrow().print_recursive(depth + 1);
        }
    }
}

impl Dentry {
    fn new(path: &Path, node: Rc<RefCell<Inode>>) -> Self {
        let metadata = path.metadata().unwrap();
        metadata.ino();
        Dentry {
            path: path.into(),
            file_name: path.file_name().unwrap().to_string_lossy().to_string(),
            file_type: metadata.file_type(),
            inode: node,
        }
    }

    fn file_name(&self) -> &str {
        &self.file_name
    }

    fn file_type(&self) -> FileType {
        self.file_type
    }
}

impl From<&Ref<'_, Inode>> for CodexFsInode {
    fn from(node: &Ref<'_, Inode>) -> Self {
        let metadata = node.path.metadata().unwrap();
        Self {
            format: CodexFsInodeFormat::CODEXFS_INODE_FLAT_PLAIN,
            mode: metadata.mode(),
            nlink: node.cf_nlink,
            size: node.size,
            blknr: codexfs_blknr(node.cf_s_off.unwrap_or(0)),
            blkoff: codexfs_blkoff(node.cf_s_off.unwrap_or(0)),
            ino: node.cf_ino,
            uid: metadata.uid(),
            gid: metadata.gid(),
            reserved: [0; 26],
        }
    }
}

impl From<&Dentry> for CodexFsDirent {
    fn from(dentry: &Dentry) -> Self {
        Self {
            nid: dentry.inode.borrow().cf_nid,
            nameoff: 0,
            file_type: dentry.file_type.into(),
            reserved: 0,
        }
    }
}

pub fn load_inode_tree(path: &Path) -> Result<Inode, std::io::Error> {
    assert!(path.is_dir());

    let mut root = Inode::new_dir(path);

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let entry_ino = entry.metadata()?.ino();

        if entry_path.is_dir() {
            let child = load_inode_tree(&entry_path)?;
            let child = Rc::new(RefCell::new(child));
            insert_inode(entry_ino, child.clone());
            let child_dentry = Dentry::new(&entry_path, child);
            root.inc_nlink();
            root.add_dentry(child_dentry);
        } else {
            let child = get_inode(entry_ino).cloned().unwrap_or_else(|| {
                let child = Inode::new(&entry_path);
                assert!(child.file_type().is_file());
                Rc::new(RefCell::new(child))
            });
            child.borrow_mut().inc_nlink();
            insert_inode(entry_ino, child.clone());
            let child_dentry = Dentry::new(&entry_path, child);
            root.add_dentry(child_dentry);
        }

        println!("{}", entry.path().to_string_lossy());
    }

    Ok(root)
}

pub fn calc_inode_off(root: &Rc<RefCell<Inode>>) {
    for dentry in root.borrow_mut().dentries.iter_mut() {
        let child = &dentry.inode;
        if child.borrow().file_type().is_dir() {
            calc_inode_off(child);
        } else {
            let mut child = child.borrow_mut();
            assert!(child.file_type().is_file());
            let start_off = get_sb().get_start_off();
            if child.cf_s_off.is_none() {
                child.cf_s_off = Some(start_off);
            }
            get_mut_sb().set_start_off(start_off + child.size);
        }
    }
}

// FIXME: dirent off should be calculated
pub fn dump_inode_tree(node: &Rc<RefCell<Inode>>) -> io::Result<()> {
    let sb = get_mut_sb();
    let file_type = node.borrow().file_type();

    if file_type.is_dir() {
        {
            let node_ref = node.borrow();
            for dentry in node_ref.dentries.iter() {
                let child = &dentry.inode;
                dump_inode_tree(child)?;
            }
            let inode = CodexFsInode::from(&node_ref);
            sb.img_file
                .write_all_at(inode.to_bytes(), sb.get_start_off())?;
            sb.inc_start_off(size_of::<CodexFsInode>() as u64);
        }
        {
            let mut node_mut = node.borrow_mut();
            node_mut.cf_nid = codexfs_nid(sb.get_start_off());
            sb.inc_start_off(size_of::<CodexFsInode>() as u64);
        }
        {
            let node_ref = node.borrow();
            for dentry in node_ref.dentries.iter() {
                let codexfs_dirent = CodexFsDirent::from(dentry);
                sb.img_file
                    .write_all_at(codexfs_dirent.to_bytes(), sb.get_start_off())?;
                sb.inc_start_off(size_of::<CodexFsDirent>() as u64);
            }
            for dentry in node_ref.dentries.iter() {
                sb.img_file
                    .write_all_at(dentry.file_name().as_bytes(), sb.get_start_off())?;
                sb.inc_start_off(dentry.file_name().len() as u64);
            }
        }
    } else if file_type.is_file() {
        {
            let node_ref = node.borrow();
            let mut file = File::open(&node_ref.path)?;
            let mut content = Vec::new();
            file.read_to_end(&mut content)?;
            sb.img_file
                .write_all_at(&content, node_ref.cf_s_off.unwrap())?;

            let codexfs_inode = CodexFsInode::from(&node_ref);
            sb.img_file
                .write_all_at(codexfs_inode.to_bytes(), sb.get_start_off())?;
        }
        {
            let mut node_mut = node.borrow_mut();
            node_mut.cf_nid = codexfs_nid(sb.get_start_off());
            sb.inc_start_off(size_of::<CodexFsInode>() as u64);
        }
    } else {
        todo!()
    }

    Ok(())
}
