use std::{cell::OnceCell, collections::HashSet, io::Write, rc::Rc};

use anyhow::{Ok, Result};
use tlsh_fixed::{BucketKind, ChecksumKind, Tlsh, TlshBuilder, Version};

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
    pub file_data: Vec<u8>,
    pub off: off_t,
    pub files: Vec<Rc<Inode<File>>>,
    pub diff_mat: Vec<Vec<usize>>,
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
        self.files.push(inode.clone());
        self.off += inode.itype.size as u64;
        log::info!("push file {}", inode.meta.path.as_ref().unwrap().display());
        Ok(())
    }

    pub fn construct_diff_map(&mut self) {
        const DEFAULT_DIFF: usize = 1000;
        let len = self.files.len();
        self.diff_mat = vec![vec![0; len]; len];
        for i in 0..len {
            for j in i + 1..len {
                let inode_pair = (&self.files[i], &self.files[j]);
                let diff = {
                    let tlsh_pair = (
                        &inode_pair.0.itype.inner.borrow().tlsh,
                        &inode_pair.1.itype.inner.borrow().tlsh,
                    );
                    match tlsh_pair {
                        (Some(t0), Some(t1)) => t0.diff(t1, false),
                        _ => DEFAULT_DIFF,
                    }
                };
                self.diff_mat[i][j] = diff;
                self.diff_mat[j][i] = diff;
            }
        }
    }

    pub fn optimize(&mut self) {
        // 生成初始路径
        let initial_path = nearest_neighbor(&self.diff_mat);
        println!("初始路径: {:?}", initial_path);

        // 优化路径
        let optimized_path = two_opt_optimize(initial_path, &self.diff_mat);
        println!("优化路径: {:?}", optimized_path);
        println!(
            "总差异: {}",
            calculate_total_cost(&optimized_path, &self.diff_mat)
        );

        let real_path = optimized_path
            .iter()
            .map(|idx| self.files[*idx].meta.path())
            .collect::<Vec<_>>();
        println!("优化路径: ");
        for path in real_path.iter() {
            println!("{}", path.display());
        }

        self.files = optimized_path
            .iter()
            .map(|idx| self.files[*idx].clone())
            .collect::<Vec<_>>();

        for file in self.files.iter() {
            self.file_data.extend(file.read_to_end().unwrap().iter());
        }
    }
}

pub fn get_tlsh(content: &[u8]) -> Option<Tlsh> {
    let mut builder = TlshBuilder::new(
        BucketKind::Bucket128,
        ChecksumKind::OneByte,
        Version::Version4,
    );
    builder.update(content);
    builder.build().ok()
}

// 选择初始节点（总差异最小的节点）
fn select_initial_node(diff_matrix: &Vec<Vec<usize>>) -> usize {
    diff_matrix
        .iter()
        .enumerate()
        .map(|(i, row)| (i, row.iter().sum::<usize>())) // 计算每行总和
        .min_by_key(|&(_, sum)| sum)                     // 找出总差异最小的行
        .unwrap().0 // 返回索引
}

// 最近邻贪心算法构建路径
fn nearest_neighbor(diff_matrix: &Vec<Vec<usize>>) -> Vec<usize> {
    let n = diff_matrix.len();
    let start = select_initial_node(diff_matrix);
    let mut path = vec![start];
    let mut unvisited: HashSet<usize> = (0..n).collect(); // 初始化未访问集合
    unvisited.remove(&start);
    let mut current = start;

    while !unvisited.is_empty() {
        // 寻找差异最小的未访问节点
        let nearest = *unvisited
            .iter()
            .min_by_key(|&&node| diff_matrix[current][node])
            .unwrap();
        path.push(nearest);
        unvisited.remove(&nearest);
        current = nearest;
    }
    path
}

// 计算路径总差异值
fn calculate_total_cost(path: &[usize], diff_matrix: &Vec<Vec<usize>>) -> usize {
    path.windows(2)
        .map(|pair| diff_matrix[pair[0]][pair[1]])
        .sum()
}

// 2-opt 优化算法
fn two_opt_optimize(mut path: Vec<usize>, diff_matrix: &Vec<Vec<usize>>) -> Vec<usize> {
    let n = path.len();
    let mut best_path = path.clone();
    let mut min_cost = calculate_total_cost(&best_path, diff_matrix);
    let mut improved = true;

    while improved {
        improved = false;
        for i in 0..n - 1 {
            for j in i + 2..n {
                // 计算交换前后的成本变化
                let a = path[i];
                let b = path[i + 1];
                let c = path[j - 1];
                let d = path[j % n]; // 处理环状路径

                // 原路径中 a-b 和 c-d 的差异
                let original = diff_matrix[a][b] + diff_matrix[c][d];
                // 交换后 a-c 和 b-d 的差异
                let new = diff_matrix[a][c] + diff_matrix[b][d];

                if new < original {
                    // 反转 i+1 到 j-1 的子路径
                    let mut new_path = path[..=i].to_vec();
                    let mut middle = path[i + 1..j].to_vec();
                    middle.reverse();
                    new_path.extend(middle);
                    new_path.extend(&path[j..]);

                    // 计算新总成本
                    let new_cost = calculate_total_cost(&new_path, diff_matrix);

                    if new_cost < min_cost {
                        best_path = new_path.clone();
                        min_cost = new_cost;
                        path = new_path;
                        improved = true;
                        break; // 发现改进后重新扫描
                    }
                }
            }
            if improved {
                break;
            }
        }
    }
    best_path
}
