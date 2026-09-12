//! 游戏存储：所有刮刮卡都放在内存里
//!
//! 这个项目**不用数据库**，所有卡片数据就存在内存的一个大 HashMap 里。
//! 这个模块就是"存卡的地方"，以及"卡片太多时怎么清理"。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::models::Game;

/// 服务器最多同时保留多少张卡。
/// 如果无限制地存下去，内存迟早会被占满，所以定一个上限。
const MAX_GAMES: usize = 2000;

/// 游戏仓库：一张 HashMap，卡片编号 → 卡片对象。
///
/// 除了存卡，还顺便统计了运行以来的总数据（这些数字暂时没有对外暴露，
/// 但保留了接口，以后想做"今日中奖排行榜"之类的功能可以直接用）：
/// - `total_cards`：一共发了多少张卡
/// - `total_wins`：一共中奖多少次
/// - `total_reward`：一共发放了多少积分
///
/// `#[derive(Default)]` 会生成 `GameStore::default()`：
/// 把每个字段都初始化为"空"（HashMap 空、数字全 0）。
/// main.rs 里 `GameStore::default()` 就是这么来的。
#[derive(Default)]
pub struct GameStore {
    games: HashMap<String, Game>,
    pub total_cards: u64,
    pub total_wins: u64,
    pub total_reward: u64,
}

impl GameStore {
    /// 往仓库里放一张新卡。
    ///
    /// 注意参数是 `game: Game`（直接拿走所有权，不再拷贝）。
    /// 调用方 new_game handler 创建完卡后就把卡"交给"仓库保管。
    pub fn insert(&mut self, game: Game) {
        // 如果卡已经达到上限，先清理一波腾出空间
        if self.games.len() >= MAX_GAMES {
            self.prune();
        }
        // 存进 HashMap：key 是卡片 id（32 位十六进制字符串）
        self.games.insert(game.id.clone(), game);
        // 统计 +1
        self.total_cards += 1;
    }

    /// 清理旧卡，腾出空间。
    /// 分两步：
    /// 1. 先清掉所有"已结算"的卡（已经没用了，留着纯占内存）
    /// 2. 如果还是超上限，就按创建时间从旧到新删掉一半
    fn prune(&mut self) {
        // retain 是 HashMap 的"按条件删除"：
        // 留下返回 true 的，删掉返回 false 的。这里留下"还没结算"的卡
        self.games.retain(|_, game| !game.finished);
        // 如果清了已结算的还不够，说明未结算的卡也太多
        if self.games.len() >= MAX_GAMES {
            // 把所有卡按"创建时间"收集起来
            let mut by_age: Vec<(String, std::time::Instant)> = self
                .games
                .iter()
                .map(|(id, g)| (id.clone(), g.created_at))
                .collect();
            // sort_by_key 按创建时间排序：最早创建的排最前面
            by_age.sort_by_key(|(_, created)| *created);
            // take(MAX_GAMES / 2) 拿走最老的一半，然后挨个从仓库里删掉。
            // 注意这里只删了一半，另外一半还留着用。
            for (id, _) in by_age.into_iter().take(MAX_GAMES / 2) {
                self.games.remove(&id);
            }
        }
    }

    /// 按卡片 id 取出那张卡的**可变引用**，方便修改它（比如刮开格子）。
    ///
    /// 为什么叫 `game_mut`（mut = mutable = 可变）：
    /// 因为返回值是 `Option<&mut Game>`，拿到的是"能改的引用"。
    /// Rust 里"读"和"写"是两种引用：`&Game` 只能读，`&mut Game` 才能改。
    ///
    /// 返回 `Option` 是因为卡片可能不存在（被清理了、id 打错了），
    /// 这时返回 None，由调用方（api.rs）转成 404 错误返回给前端。
    pub fn game_mut(&mut self, card_id: &str) -> Option<&mut Game> {
        self.games.get_mut(card_id)
    }

    /// 记录一次结算的结果，更新统计数字。
    /// `record_finish` 里的 finish 就是"结算"的意思。
    ///
    /// 只有 reward > 0（真的中奖了）才把中奖次数和总积分累加，
    /// 没中奖（reward == 0）就不用更新这两个数字了。
    pub fn record_finish(&mut self, reward: u32) {
        if reward > 0 {
            self.total_wins += 1;
            self.total_reward += u64::from(reward);
        }
    }
}

/// 全项目的共享仓库类型。
///
/// 这是理解本项目的关键一行，拆开看：
///
/// 1. **`Arc<RwLock<T>>` 是什么？**
///    服务器同时要处理很多浏览器发来的请求，也就是多线程并发。
///    但多个线程不能同时乱改同一份数据，Rust 用锁来保护。
///    - `RwLock`：读写锁。"读锁"可以好几个人同时拿（不冲突），
///      "写锁"只能一个人拿（其他人得等着）。读多写少的场景最合适。
///    - `Arc`：原子引用计数。多个线程要"共用"同一个 RwLock，
///      不能各自复制一份（那就不是同一份数据了），所以用 Arc 共享所有权。
///      每当有人克隆一个 Arc，计数 +1；用完了计数 -1，归零时数据才被销毁。
///
/// 2. **怎么用？**
///    - 读：`store.read().unwrap()` → 得到读锁，只能读不能写
///    - 写：`store.write().unwrap()` → 得到写锁，独占
///    - `.clone()` Arc 的克隆非常便宜（只加个计数），所以可以放心到处克隆
///
/// 3. **为什么写在 type 别名里？**
///    后面 api.rs 的 handler 都要用这个类型当参数，
///    起个短名字 `SharedStore` 就不用每次写一长串。
pub type SharedStore = Arc<RwLock<GameStore>>;
