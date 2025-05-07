use std::{
    cell::OnceCell,
    collections::{HashSet, VecDeque},
    rc::Rc,
};

use tlsh_fixed::{BucketKind, ChecksumKind, Tlsh, TlshBuilder, Version};

use crate::inode::{File, Inode};

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

    pub fn reorder(&mut self) {
        self.construct_diff_map();
        self.optimize();
        for file in self.files.iter() {
            self.file_data
                .extend(file.itype.inner.borrow().content.as_ref().unwrap());
        }
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
                    log::debug!("tlsh pair {:?}", tlsh_pair);
                    match tlsh_pair {
                        (Some(t0), Some(t1)) => t0.diff(t1, false),
                        _ => DEFAULT_DIFF,
                    }
                };
                log::info!(
                    "diff of {} and {} is {}",
                    inode_pair.0.meta.path().display(),
                    inode_pair.1.meta.path().display(),
                    diff
                );
                self.diff_mat[i][j] = diff;
                self.diff_mat[j][i] = diff;
            }
        }
    }

    pub fn optimize(&mut self) {
        // let initial_path = nearest_neighbor(&self.diff_mat);
        let initial_path = nearest_neighbor_dual_end(&self.diff_mat);

        let optimized_path = two_opt_optimize(initial_path, &self.diff_mat);
        log::info!(
            "total cost: {}",
            calculate_total_cost(&optimized_path, &self.diff_mat)
        );

        let real_path = optimized_path
            .iter()
            .map(|idx| self.files[*idx].meta.path())
            .collect::<Vec<_>>();
        log::info!("path reordered: ");
        for path in real_path.iter() {
            log::info!("{}", path.display());
        }

        self.files = optimized_path
            .iter()
            .map(|idx| self.files[*idx].clone())
            .collect::<Vec<_>>();
    }
}

pub fn calc_tlsh(content: &[u8]) -> Option<Tlsh> {
    let mut builder = TlshBuilder::new(
        BucketKind::Bucket256,
        ChecksumKind::ThreeByte,
        Version::Version4,
    );
    builder.update(content);
    builder.build().ok()
}

fn select_initial_node(diff_mat: &[Vec<usize>]) -> usize {
    diff_mat
        .iter()
        .enumerate()
        .map(|(i, row)| (i, row.iter().sum::<usize>()))
        .min_by_key(|&(_, sum)| sum)
        .unwrap()
        .0
}

fn nearest_neighbor(diff_mat: &[Vec<usize>]) -> Vec<usize> {
    let n = diff_mat.len();
    let start = select_initial_node(diff_mat);
    let mut path = vec![start];
    let mut unvisited: HashSet<usize> = (0..n).collect();
    unvisited.remove(&start);
    let mut current = start;

    while !unvisited.is_empty() {
        let nearest = *unvisited
            .iter()
            .min_by_key(|&&node| diff_mat[current][node])
            .unwrap();
        path.push(nearest);
        unvisited.remove(&nearest);
        current = nearest;
    }
    path
}

fn nearest_neighbor_dual_end(diff_mat: &[Vec<usize>]) -> Vec<usize> {
    let n = diff_mat.len();
    let start = select_initial_node(diff_mat);
    let mut path = VecDeque::new();
    path.push_back(start);
    let mut unvisited: HashSet<usize> = (0..n).collect();
    unvisited.remove(&start);
    let mut front = start;
    let mut back = start;

    while !unvisited.is_empty() {
        let nearest_front = unvisited
            .iter()
            .min_by_key(|&&node| diff_mat[front][node])
            .copied();
        let nearest_back = unvisited
            .iter()
            .min_by_key(|&&node| diff_mat[back][node])
            .copied();

        let (candidate, is_front) = match (nearest_front, nearest_back) {
            (Some(nf), Some(nb)) => {
                if diff_mat[front][nf] <= diff_mat[back][nb] {
                    (nf, true)
                } else {
                    (nb, false)
                }
            }
            (Some(nf), None) => (nf, true),
            (None, Some(nb)) => (nb, false),
            (None, None) => unreachable!(),
        };

        if is_front {
            path.push_front(candidate);
            front = candidate;
        } else {
            path.push_back(candidate);
            back = candidate;
        }
        unvisited.remove(&candidate);
    }

    path.into_iter().collect()
}

fn calculate_total_cost(path: &[usize], diff_mat: &[Vec<usize>]) -> usize {
    path.windows(2).map(|pair| diff_mat[pair[0]][pair[1]]).sum()
}

// 2-opt 优化算法
fn two_opt_optimize(mut path: Vec<usize>, diff_matrix: &[Vec<usize>]) -> Vec<usize> {
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
