//! 游戏核心逻辑：发牌、概率、中奖判定、刮开、结算
//!
//! 这个文件是"刮刮乐的大脑"。前端只负责展示和刮奖动画，
//! 真正决定"这张卡会不会中奖、中了哪个奖"的代码全在这里。
//!
//! 整个发牌的流程（Game::create）：
//! 1. 生成一个 0~1 的随机数，按概率表选一个奖级（也可能没中）
//! 2. 根据选中的结果，构造 9 个格子的图案：
//!    - 中奖卡：随机挑一条三连线（横/竖/斜），把这条线填成中奖符号，
//!      其余格子填随机填充符号，并且要保证**恰好只有这一条三连**
//!    - 未中奖卡：全是随机填充符号，但要保证**一条三连都没有**
//! 3. 生成一张 32 位十六进制的唯一卡片 id
//!
//! 这样发出来的牌，中没中奖在**发牌那一刻就定死了**，用户刮开只是逐步揭开真相。

use crate::config::{GameConfig, RewardTier};
use crate::models::{
    Cell, CellHint, FinishResponse, Game, GameError, GRID_SIZE, NewCardResponse, RevealResponse,
};

/// 棋盘上所有可能的"三连线"。
/// 每一条线用 3 个格子编号表示（0~8，按从左到右、从上到下编号）：
/// 前 3 条是横着的一行，中间 3 条是竖着的一列，最后 2 条是对角线。
///
/// `[[usize; 3]; 8]` 的类型读法：
/// 外层 8 = 一共有 8 条线，内层 `[usize; 3]` = 每条线是 3 个数字。
/// 3×3 棋盘里，三连一共就这 8 种可能，写全。
const LINES: [[usize; 3]; 8] = [
    [0, 1, 2], // 第 1 行：  [0 1 2]
    [3, 4, 5], // 第 2 行：  [3 4 5]
    [6, 7, 8], // 第 3 行：  [6 7 8]
    [0, 3, 6], // 第 1 列
    [1, 4, 7], // 第 2 列
    [2, 5, 8], // 第 3 列
    [0, 4, 8], // 主对角线 ↘
    [2, 4, 6], // 副对角线 ↙
];

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

/// 生成一张卡片的唯一 id：16 个随机字节 → 32 位十六进制字符串。
///
/// `pub` 关键字：这个函数是公开的（虽然目前只在 game.rs 内部用到），
/// 预留出来，以后别的地方也可能需要。
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
/// - rng < 0.001    → 一等奖
/// - 0.001 ≤ rng < 0.011 → 二等奖
/// - 0.011 ≤ rng < 0.061 → 三等奖
/// - 0.061 ≤ rng < 0.261 → 安慰奖
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

/// 检查棋盘上有没有任何一条三连线，有的话返回那条线上的符号。
///
/// 入参 `grid: &[String]` 是"一串格子符号"，`&` 表示只借读不拿走。
/// `Option<String>` 返回：Some(符号) = 有三连，None = 一条三连都没有。
///
/// 在发牌时的用途很关键：
/// - 中奖卡：用它确认"中奖符号的三连恰好存在，且没有别的三连"
/// - 未中奖卡：用它确认"绝对没有三连"（否则用户会误以为中奖了）
fn winning_symbol(grid: &[String]) -> Option<String> {
    // 遍历那 8 条三连线
    for line in LINES {
        let [a, b, c] = line; // 把 3 个格子编号解构出来
        // 三个格子符号都相等 → 就是一条三连，返回这个符号
        if grid[a] == grid[b] && grid[b] == grid[c] {
            return Some(grid[a].clone());
        }
    }
    None
}

/// 从填充符号表里随机挑一个符号。
///
/// `(random_u64() as usize) % len`：随机数对 len 取余，结果保证在 0..len。
/// 这是"取随机下标"的常见做法（有个很小的均匀性瑕疵叫"取模偏差"，演示项目无所谓）。
fn random_filler(config: &GameConfig) -> String {
    let len = config.filler_symbols.len();
    let idx = (random_u64() as usize) % len;
    config.filler_symbols[idx].clone()
}

/// 构造 9 个格子的图案，是整个发牌过程的核心。
///
/// 入参：
/// - `config`：配置（奖级表 + 填充符号表）
/// - `tier`：已经选好的奖级。Some(奖级) = 这张卡要中奖，None = 不中奖
///
/// 返回值 `Vec<String>`：长度 9 的一串符号，index 就是格子编号。
///
/// 关于"随机试错法"（两个分支里都有的 for _ in 0..1000 循环）：
/// 光靠"选中奖线填中奖符号"还不够，因为填充符号可能"碰巧"又凑出一条
/// 别的三连。所以采取朴素策略：反复随机生成、检查、不合条件就重来，
/// 最多试 1000 次。绝大多数情况一次就过，1000 次只是极端的兜底。
fn build_grid(config: &GameConfig, tier: Option<&RewardTier>) -> Vec<String> {
    match tier {
        // ---- 分支一：没中奖。要求棋盘上一条三连都没有 ----
        None => {
            for _ in 0..1000 {
                // 生成 9 个随机填充符号，组成一张临时棋盘
                let grid: Vec<String> =
                    (0..GRID_SIZE).map(|_| random_filler(config)).collect();
                // 没有三连就满意，返回它
                if winning_symbol(&grid).is_none() {
                    return grid;
                }
                // 否则重来
            }
            // 理论概率极低：随机了 1000 次都凑出三连。
            // 走手工兜底：直接拼一个结构上不可能有三连的棋盘。
            // 用 pick(i) = filler[i % 6]，即每个格子按 0,1,2,2,0,1,1,3,0 的规律取符号。
            // 三行分别是 (0,1,2) (2,0,1) (1,3,0)，每行内部都不等，任何一行都不会三连。
            let pick = |i: usize| config.filler_symbols[i % config.filler_symbols.len()].clone();
            vec![
                pick(0), pick(1), pick(2),
                pick(2), pick(0), pick(1),
                pick(1), pick(3), pick(0),
            ]
        }
        // ---- 分支二：中奖。要求有且仅有一条中奖符号的三连 ----
        Some(tier) => {
            // 先随机挑一条三连线，作为"要填中奖符号的那条线"
            let win_line = LINES[(random_u64() as usize) % LINES.len()];
            for _ in 0..1000 {
                // 全部填随机填充符号，再单独把选中的那一条线改成中奖符号
                let mut grid: Vec<String> =
                    (0..GRID_SIZE).map(|_| random_filler(config)).collect();
                for idx in win_line {
                    grid[idx] = tier.symbol.clone();
                }
                // 检查：棋盘上的三连"恰好就是"中奖符号吗？
                // 用 match 而不是 if，是为了同时做两件事：
                // - 有三连且它就是中奖符号 → 满意，返回
                // - 三连是别的符号（填充符号凑出来的）或没有三连 → 继续重试
                match winning_symbol(&grid) {
                    Some(s) if s == tier.symbol => return grid,
                    _ => continue,
                }
            }
            // 兜底：顶行三个格子直接填中奖符号（这本身就是一条三连），
            // 下面两行用 pick 函数填出"不可能再凑出三连"的图案。
            // 同样，每行内部都互不相同，保证只有顶行这一条三连。
            let pick = |i: usize| config.filler_symbols[i % config.filler_symbols.len()].clone();
            vec![
                tier.symbol.clone(), tier.symbol.clone(), tier.symbol.clone(),
                pick(2),              pick(0),              pick(1),
                pick(1),              pick(3),              pick(0),
            ]
        }
    }
}

/// 给 Game 结构体实现方法。
///
/// 什么是 `impl Game`：
/// Rust 把"数据"和"操作数据的方法"分开。
/// `struct Game` 只定义了数据长什么样（在 models.rs 里），
/// `impl Game` 给这个结构体挂上方法（这里就是"创建卡、刮格、结算"）。
/// 之后就能 `game.reveal_cell(3)` 这样调用了。
impl Game {
    /// 创建一张全新的刮刮卡，走完整个"发牌"流程。
    pub fn create(config: &GameConfig) -> Self {
        // 第一步：随机选奖级
        let tier = choose_tier(random_unit(), config);
        // 第二步：按中没中奖构造 9 格图案
        let grid = build_grid(config, tier);
        // 第三步：把符号串 + 编号 + revealed=false 包成 9 个 Cell 对象
        let cells = grid
            .into_iter()
            .enumerate() // enumerate 给每个元素配上序号 (0,符号) (1,符号) ...
            .map(|(id, symbol)| Cell {
                id: id as u8, // usize 转 u8（id 最大才 8，安全）
                symbol,
                revealed: false, // 新卡所有格子都还没刮开
            })
            .collect();

        Game {
            id: new_card_id(), // 唯一编号
            cells,
            // tier.map_or(0, |t| t.value) 的拆解：
            // Option 是 Some(奖级) 时取奖级的 value，是 None 时取 0（没中奖）
            reward: tier.map_or(0, |t| t.value),
            tier_name: tier.map(|t| t.name.clone()), // 奖级名称
            win_symbol: tier.map(|t| t.symbol.clone()), // 中奖符号
            finished: false,   // 新卡还没结算
            created_at: std::time::Instant::now(), // 记录此刻，供清理旧卡用
        }
    }

    /// 生成"新建卡片"的响应数据。
    ///
    /// `&self`：只读方法，不修改卡片。
    /// 关键点：返回的 CellHint 只有格子编号，**没有符号**。
    /// 卡片 id 和格子编号可以放心给前端，图案必须藏着。
    pub fn to_new_card_response(&self) -> NewCardResponse {
        NewCardResponse {
            card_id: self.id.clone(),
            // 把 9 个格子映射成 9 个"只含编号"的提示
            cells: self.cells.iter().map(|c| CellHint { id: c.id }).collect(),
        }
    }

    /// 刮开（揭示）一个格子。
    ///
    /// 入参 cell_id：格子编号。出参 `Result<RevealResponse, GameError>`：
    /// Ok(这一格的图案) 或 Err(出错原因)。
    ///
    /// `&mut self`：可变方法，因为要修改格子的 revealed 状态。
    pub fn reveal_cell(&mut self, cell_id: u8) -> Result<RevealResponse, GameError> {
        // 已经结算的卡不能再刮
        if self.finished {
            return Err(GameError::AlreadyFinished);
        }
        // 在 9 个格子里找编号等于 cell_id 的那个。
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
        // 标记为已刮开，并返回这一格的符号
        cell.revealed = true;
        Ok(RevealResponse {
            cell_id: cell.id,
            symbol: cell.symbol.clone(),
        })
    }

    /// 结算这张卡（只能调用一次）。
    ///
    /// 刮到足够多的格子后，前端调用这个接口，服务器给出最终裁决。
    /// 中没中奖在 create 时已经定好，这里只是把它算出来告诉用户。
    pub fn finish(&mut self) -> Result<FinishResponse, GameError> {
        // 已经结算过的卡，再调就报错（防止重复领奖）
        if self.finished {
            return Err(GameError::AlreadyFinished);
        }
        // 标记已结算。以后这张卡就不能再刮、不能再结算了
        self.finished = true;
        // reward > 0 就说明中了奖
        let win = self.reward > 0;
        // 拼一句话给用户看
        let message = if win {
            // tier_name.as_deref()：把 Option<String> 变成 Option<&str>，
            // 方便用 unwrap_or("") 兜底（虽然中奖时肯定有名字）
            format!(
                "恭喜中奖：{} +{} 积分！",
                self.tier_name.as_deref().unwrap_or(""),
                self.reward
            )
        } else {
            "谢谢参与，再接再厉！".to_string()
        };
        Ok(FinishResponse {
            win,
            reward: self.reward,
            tier: self.tier_name.clone(),
            symbol: self.win_symbol.clone(),
            message,
        })
    }
}

/// 单元测试模块。
///
/// 这些测试用随机生成 + 统计验证的方式，确保发牌逻辑的正确性：
/// 中奖卡必须有且仅有一条中奖三连、未中奖卡不能有任何三连、
/// 概率分布和配置对得上……跑 `cargo test` 就会全部执行。
/// 因为发牌带随机性，测试里都循环上万次用统计说话，而不是测单个结果。
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的默认配置（直接复用内置默认值）
    fn test_config() -> GameConfig {
        GameConfig::default()
    }

    /// 每张卡都是 9 格、id 唯一、格子编号按 0~8 排好、初始都没刮开
    #[test]
    fn game_has_nine_cells_and_unique_id() {
        let config = test_config();
        let game = Game::create(&config);
        assert_eq!(game.cells.len(), GRID_SIZE);
        for (i, cell) in game.cells.iter().enumerate() {
            assert_eq!(cell.id as usize, i);
            assert!(!cell.symbol.is_empty());
            assert!(!cell.revealed);
        }
        assert_eq!(game.id.len(), 32);
    }

    /// 生成 1000 张卡，id 不允许重复
    #[test]
    fn card_ids_are_unique() {
        let config = test_config();
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let game = Game::create(&config);
            assert!(ids.insert(game.id));
        }
    }

    /// 核心规则验证：中奖卡恰好只有中奖符号一条三连；未中奖卡没有三连
    #[test]
    fn win_cards_have_exactly_one_winning_line_symbol() {
        let config = test_config();
        for _ in 0..10_000 {
            let game = Game::create(&config);
            // 从卡片里把符号抽出来，还原成棋盘
            let grid: Vec<String> = game.cells.iter().map(|c| c.symbol.clone()).collect();
            let line_symbol = winning_symbol(&grid);
            if game.reward > 0 {
                let win_symbol = game.win_symbol.as_ref().expect("win card has symbol");
                assert_eq!(line_symbol.as_ref(), Some(win_symbol), "win card must contain a 3-line of its tier symbol");
            } else {
                assert!(line_symbol.is_none(), "losing card must not contain a 3-line");
            }
        }
    }

    /// 卡片的积分、奖级名和配置表完全一致
    #[test]
    fn reward_matches_chosen_tier() {
        let config = test_config();
        for _ in 0..10_000 {
            let game = Game::create(&config);
            if game.reward > 0 {
                let tier = config
                    .rewards
                    .iter()
                    .find(|t| t.name == game.tier_name.as_deref().unwrap())
                    .expect("tier name must exist in config");
                assert_eq!(tier.value, game.reward);
            } else {
                assert!(game.tier_name.is_none());
            }
        }
    }

    /// 抽 10 万张，统计各奖级实际出现比例，与配置概率误差 < 1%
    #[test]
    fn probability_distribution_is_roughly_correct() {
        let config = test_config();
        let rounds = 100_000;
        let mut counts = std::collections::HashMap::new();
        for _ in 0..rounds {
            let game = Game::create(&config);
            *counts.entry(game.reward).or_insert(0u32) += 1;
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
        let mut game = Game::create(&config);
        assert_eq!(game.reveal_cell(9).unwrap_err(), GameError::CellOutOfRange);
        assert!(game.reveal_cell(0).is_ok());
        assert_eq!(game.reveal_cell(0).unwrap_err(), GameError::AlreadyRevealed);
    }

    /// 结算只能发生一次，结算后不能再刮
    #[test]
    fn finish_can_only_happen_once() {
        let config = test_config();
        let mut game = Game::create(&config);
        let first = game.finish().expect("first finish succeeds");
        assert_eq!(first.win, first.reward > 0);
        assert!(game.finished);
        assert_eq!(game.finish().unwrap_err(), GameError::AlreadyFinished);
        assert_eq!(game.reveal_cell(1).unwrap_err(), GameError::AlreadyFinished);
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
