//! 游戏核心逻辑：发牌（每格中多少钱）、刮开、结算
//!
//! 这个文件是"刮刮乐的大脑"。前端只负责展示和刮奖动画，
//! 真正决定"这一格会不会中奖、中了多少钱"的代码全在这里。
//!
//! 整个发牌的流程（Game::create）：
//! 1. 这一页要几个刮奖格由调用方决定（最多 MAX_PAGE_CELLS 格）
//! 2. 每一格**独立**生成一个 0~1 的随机数，按概率表选一个奖级：
//!    - 中奖：记下金额（如 20）和显示文本（如 "$20"）
//!    - 没中：金额 0，显示文本取配置里的 lose_label（如 "$0"）
//! 3. 生成一个 32 位十六进制的唯一页面 id
//!
//! 这样发出来的牌，中没中奖在**发牌那一刻就定死了**，
//! 用户刮开只是逐步揭开真相，前端改不了结果。

use crate::config::{GameConfig, RewardTier};
use crate::models::{
    Cell, CellHint, FinishResponse, Game, GameError, MAX_PAGE_CELLS, NewCardResponse,
    RevealResponse,
};

/// 从操作系统拿 8 个随机字节，拼成一个 u64。
///
/// 为什么不用常见的 rand 库：
/// rand 库方便，但 getrandom 更轻量，直接用操作系统提供的真随机数，
/// 对这个"演示级游戏"来说既简单又足够随机。
///
/// `.expect("OS RNG should be available")` 的含义：
/// getrandom 理论上可能失败（极少数嵌入式环境没有熵源），
/// 这里假设它永远成功，万一失败就直接 panic（崩溃）——
/// 真随机数拿不到，游戏没法公正发牌，崩溃比继续跑更合理。
fn random_u64() -> u64 {
    let mut buf = [0u8; 8]; // 8 个字节的数组，先全填 0
    getrandom::fill(&mut buf).expect("OS RNG should be available"); // 让系统往 buf 里填真随机字节
    u64::from_le_bytes(buf) // 把 8 个字节按小端序拼成一个 u64 整数
}

/// 生成一个 0（含）到 1（不含）之间的随机小数。
///
/// 原理：u64 有 64 位，取高 53 位出来当"分子"，
/// 除以 2 的 53 次方（1u64 << 53），结果就落在 [0, 1) 区间。
/// 取 53 位是刻意为之：这是 f64 能精确表示的位数，能保证分布均匀。
/// `>> 11` 就是"右移 11 位"，把低 11 位扔掉，留下高 53 位。
fn random_unit() -> f64 {
    (random_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// 生成一页卡片的唯一 id：16 个随机字节 → 32 位十六进制字符串。
///
/// 16 字节的随机数发生碰撞的概率可以忽略不计，所以 id 足够唯一。
pub fn new_card_id() -> String {
    let mut buf = [0u8; 16]; // 16 个字节
    getrandom::fill(&mut buf).expect("OS RNG should be available");
    let mut hex = String::with_capacity(32); // 预分配 32 字符的空间，避免频繁扩容
    for byte in buf {
        // "{byte:02x}"：把 1 个字节格式化成 2 位十六进制（不足补 0），如 0a, ff
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// 用随机数 rng 按概率表选奖级。
///
/// 核心算法叫"累积概率"：
/// 把所有奖级的概率从小到大累加，随机数落在哪一段就选哪个奖。
/// 举例（概率 0.001 / 0.01 / 0.05 / 0.2）：
/// - rng < 0.001    → 特等奖
/// - 0.001 ≤ rng < 0.011 → 一等奖
/// - 0.011 ≤ rng < 0.061 → 二等奖
/// - 0.061 ≤ rng < 0.261 → 三等奖
/// - 0.261 ≤ rng < 1     → None（没中奖）
///
/// 返回 `Option<&RewardTier>`：Some(奖级) 或 None（没中奖）。
/// 注意返回的是**引用**（&），因为不需要复制奖级数据，借用配置里的就行。
fn choose_tier(rng: f64, config: &GameConfig) -> Option<&RewardTier> {
    let mut cumulative = 0.0; // 累积概率，从 0 开始往上加
    for tier in &config.rewards {
        cumulative += tier.probability; // 加上这个奖的概率
        if rng < cumulative {
            // 随机数落进"这个奖的区间"里了
            return Some(tier);
        }
    }
    // 循环走完都没命中，说明 rng 落在总和之外 → 没中奖
    None
}

/// 给 Game 结构体实现方法。
///
/// 什么是 `impl Game`：
/// Rust 把"数据"和"操作数据的方法"分开。
/// `struct Game` 只定义了数据长什么样（在 models.rs 里），
/// `impl Game` 给这个结构体挂上方法（这里就是"创建、刮格、结算"）。
/// 之后就能 `game.reveal_cell(3)` 这样调用了。
impl Game {
    /// 创建新的一页刮刮卡。
    ///
    /// `requested` 是前端想要的格子数（比如这一页只剩 3 次可刮就传 3）。
    /// 会被夹到 `1..=min(config.page_size, MAX_PAGE_CELLS)`：
    /// 既防住前端传 0 或超大值，也保证一页不会超过界面能放下的格子数。
    ///
    /// 每一格的奖级是**独立**抽的（一页里可能好几格都中奖，也可能全没中）。
    pub fn create(config: &GameConfig, requested: usize) -> Self {
        let page_size = (config.page_size as usize).clamp(1, MAX_PAGE_CELLS);
        let count = requested.clamp(1, page_size);

        // 第一步：先把每一格的奖级抽出来（None = 这一格没中）
        let tiers: Vec<Option<&RewardTier>> = (0..count)
            .map(|_| choose_tier(random_unit(), config))
            .collect();

        // 第二步：把奖级翻译成"金额 + 显示文本 + 编号 + 未刮开"的格子
        let cells: Vec<Cell> = tiers
            .into_iter()
            .enumerate() // enumerate 给每个元素配上序号 (0, 奖级) (1, 奖级) ...
            .map(|(id, tier)| {
                let (amount, label) = match tier {
                    Some(tier) => (tier.value, tier.symbol.clone()),
                    None => (0, config.lose_label.clone()),
                };
                Cell {
                    id: id as u8, // usize 转 u8（id 最大才 4，安全）
                    amount,
                    label,
                    revealed: false, // 新的一页所有格子都还没刮开
                }
            })
            .collect();

        Game {
            id: new_card_id(), // 唯一编号
            cells,
            finished: false,                       // 还没结算
            created_at: std::time::Instant::now(), // 记录此刻，供清理旧页用
        }
    }

    /// 生成"新建一页"的响应数据。
    ///
    /// `&self`：只读方法，不修改卡片。
    /// 关键点：返回的 CellHint 只有格子编号，**没有金额**。
    /// 页面 id 和格子编号可以放心给前端，金额必须藏着。
    pub fn to_new_card_response(&self) -> NewCardResponse {
        NewCardResponse {
            card_id: self.id.clone(),
            // 把每个格子映射成"只含编号"的提示
            cells: self.cells.iter().map(|c| CellHint { id: c.id }).collect(),
        }
    }

    /// 刮开（揭示）一个格子。
    ///
    /// 入参 cell_id：格子编号。出参 `Result<RevealResponse, GameError>`：
    /// Ok(这一格的金额) 或 Err(出错原因)。
    ///
    /// `&mut self`：可变方法，因为要修改格子的 revealed 状态。
    pub fn reveal_cell(&mut self, cell_id: u8) -> Result<RevealResponse, GameError> {
        // 已经结算的页面不能再刮
        if self.finished {
            return Err(GameError::AlreadyFinished);
        }
        // 在这一页的格子里找编号等于 cell_id 的那个。
        // iter_mut() 是"可变的迭代器"，因为待会要修改找到的格子。
        // find 找不到会返回 None，用 ok_or 把它转成 CellOutOfRange 错误。
        let cell = self
            .cells
            .iter_mut()
            .find(|c| c.id == cell_id)
            .ok_or(GameError::CellOutOfRange)?;
        // 这一格已经刮过了，不能重复刮
        if cell.revealed {
            return Err(GameError::AlreadyRevealed);
        }
        // 标记为已刮开，并返回这一格的金额
        cell.revealed = true;
        Ok(RevealResponse {
            cell_id: cell.id,
            amount: cell.amount,
            label: cell.label.clone(),
            win: cell.amount > 0,
        })
    }

    /// 结算这一页（只能调用一次）。
    ///
    /// 把**已刮开**格子的金额加起来，就是用户这一页真正拿到的钱。
    /// 中没中奖在 create 时已经定好，这里只是把它算出来告诉用户。
    pub fn finish(&mut self) -> Result<FinishResponse, GameError> {
        // 已经结算过的页面，再调就报错（防止重复领奖）
        if self.finished {
            return Err(GameError::AlreadyFinished);
        }
        // 标记已结算。以后这一页就不能再刮、不能再结算了
        self.finished = true;
        // 只统计刮开过的格子：没刮开的不算数
        let reward: u32 = self
            .cells
            .iter()
            .filter(|c| c.revealed)
            .map(|c| c.amount)
            .sum();
        // reward > 0 就说明中了奖
        let win = reward > 0;
        // 拼一句话给用户看
        let message = if win {
            format!("本页共中奖 ${reward}！")
        } else {
            "谢谢参与，再接再厉！".to_string()
        };
        Ok(FinishResponse {
            win,
            reward,
            message,
        })
    }
}

/// 单元测试模块。
///
/// 这些测试用随机生成 + 统计验证的方式，确保发牌逻辑的正确性：
/// 页面格子数正确、金额和配置对得上、概率分布对得上、重复刮/越界会被拒绝……
/// 跑 `cargo test` 就会全部执行。
/// 因为发牌带随机性，测试里都循环上万次用统计说话，而不是测单个结果。
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的默认配置（直接复用内置默认值）
    fn test_config() -> GameConfig {
        GameConfig::default()
    }

    /// 一页是几格、id 唯一、格子编号按 0~n 排好、初始都没刮开
    #[test]
    fn page_has_requested_cells_and_unique_id() {
        let config = test_config();
        let game = Game::create(&config, 5);
        assert_eq!(game.cells.len(), 5);
        for (i, cell) in game.cells.iter().enumerate() {
            assert_eq!(cell.id as usize, i);
            assert!(!cell.label.is_empty());
            assert!(!cell.revealed);
        }
        assert_eq!(game.id.len(), 32);
    }

    /// 一页最多 MAX_PAGE_CELLS 格，最少 1 格（挡住前端的非法请求）
    #[test]
    fn page_size_is_clamped() {
        let config = test_config();
        assert_eq!(Game::create(&config, 999).cells.len(), MAX_PAGE_CELLS);
        assert_eq!(Game::create(&config, 0).cells.len(), 1);
    }

    /// 生成 1000 页，id 不允许重复
    #[test]
    fn card_ids_are_unique() {
        let config = test_config();
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let game = Game::create(&config, 5);
            assert!(ids.insert(game.id));
        }
    }

    /// 每格的金额和显示文本都能在配置表里对上；
    /// 没中奖的格子金额必须是 0，文本必须是 lose_label
    #[test]
    fn cell_amounts_match_config() {
        let config = test_config();
        for _ in 0..2000 {
            let game = Game::create(&config, 5);
            for cell in &game.cells {
                if cell.amount == 0 {
                    assert_eq!(cell.label, config.lose_label);
                } else {
                    let tier = config
                        .rewards
                        .iter()
                        .find(|t| t.value == cell.amount)
                        .expect("winning amount must exist in config");
                    assert_eq!(cell.label, tier.symbol);
                }
            }
        }
    }

    /// 抽 10 万次，统计各奖级实际出现比例，与配置概率误差 < 1%
    #[test]
    fn probability_distribution_is_roughly_correct() {
        let config = test_config();
        let rounds = 100_000;
        let mut counts = std::collections::HashMap::new();
        for _ in 0..rounds {
            let amount = choose_tier(random_unit(), &config).map_or(0, |t| t.value);
            *counts.entry(amount).or_insert(0u32) += 1;
        }
        for tier in &config.rewards {
            let observed = counts.get(&tier.value).copied().unwrap_or(0) as f64 / rounds as f64;
            let expected = tier.probability;
            assert!(
                (observed - expected).abs() < 0.01,
                "{} observed {observed}, expected ~{expected}",
                tier.name
            );
        }
    }

    /// 刮格子的边界：越界报错、重复刮报错
    #[test]
    fn reveal_validates_range_and_duplicates() {
        let config = test_config();
        let mut game = Game::create(&config, 5);
        assert_eq!(game.reveal_cell(9).unwrap_err(), GameError::CellOutOfRange);
        assert!(game.reveal_cell(0).is_ok());
        assert_eq!(game.reveal_cell(0).unwrap_err(), GameError::AlreadyRevealed);
    }

    /// 结算只能发生一次，结算后不能再刮
    #[test]
    fn finish_can_only_happen_once() {
        let config = test_config();
        let mut game = Game::create(&config, 5);
        let first = game.finish().expect("first finish succeeds");
        assert_eq!(first.win, first.reward > 0);
        assert!(game.finished);
        assert_eq!(game.finish().unwrap_err(), GameError::AlreadyFinished);
        assert_eq!(game.reveal_cell(1).unwrap_err(), GameError::AlreadyFinished);
    }

    /// 结算金额只算刮开过的格子
    #[test]
    fn finish_sums_only_revealed_amounts() {
        let config = test_config();
        let mut game = Game::create(&config, 5);
        // 一格都没刮：结算金额必须是 0
        assert_eq!(game.finish().expect("finish").reward, 0);

        let mut game = Game::create(&config, 5);
        // 全刮开：结算金额等于所有格子金额之和
        let expected: u32 = game.cells.iter().map(|c| c.amount).sum();
        for id in 0..game.cells.len() as u8 {
            game.reveal_cell(id).expect("reveal");
        }
        let response = game.finish().expect("finish");
        assert_eq!(response.reward, expected);
        assert_eq!(response.win, expected > 0);
    }

    /// 刮开的响应必须和格子里的金额一致
    #[test]
    fn reveal_returns_cell_amount() {
        let config = test_config();
        let mut game = Game::create(&config, 5);
        let expected = game.cells[2].clone();
        let response = game.reveal_cell(2).expect("reveal");
        assert_eq!(response.cell_id, 2);
        assert_eq!(response.amount, expected.amount);
        assert_eq!(response.label, expected.label);
        assert_eq!(response.win, expected.amount > 0);
    }

    /// 选奖级函数对边界随机数的行为：0 命中第一档、刚好 1.0 未中奖
    #[test]
    fn tier_selection_respects_probabilities() {
        let config = test_config();
        assert!(choose_tier(0.0, &config).is_some());
        let first = config.rewards[0].probability;
        assert_eq!(
            choose_tier(first - f64::EPSILON, &config).unwrap().name,
            config.rewards[0].name
        );
        assert!(choose_tier(1.0, &config).is_none());
    }
}
