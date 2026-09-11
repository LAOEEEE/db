//! 数据模型与 API 报文
//!
//! 这个文件是整个项目的"数据字典"：
//! 定义刮刮卡、格子长什么样，以及前端和后端之间收发 JSON 报文的结构。
//! 你在浏览器里看到的接口返回什么字段，都是由这里的结构体决定的。

use serde::{Deserialize, Serialize};

/// 刮刮卡棋盘的格子总数。
/// 因为棋盘是 3×3 的方格，所以一共 9 格。
/// 用常量而不是到处写死数字 9，以后想改成 4×4 只改这一处。
pub const GRID_SIZE: usize = 9;

/// 棋盘上的一个格子。
///
/// 三个字段的含义：
/// - `id`：格子编号，范围 0~8，用来定位"刮开的是哪一格"
/// - `symbol`：格子里的图案，是一个 emoji 字符串，比如 💎 ⭐ 🍒
/// - `revealed`：这格是否已经被刮开（一开始全是 false）
///
/// 关于 `#[derive(...)]`：
/// 给结构体"白送"一些常用能力的魔法语法，写在括号里的都是要自动实现的东西：
/// - `Debug`：可以 println!("{:?}") 打印出来，方便调试
/// - `Clone`：可以复制出一份，不用担心所有权被拿走
/// - `Serialize`：能把结构体转成 JSON（Rust 的名字叫 serde 序列化）
/// - `Deserialize`：能从 JSON 转回结构体（serde 反序列化）
/// 前端发的请求要能被反序列化，后端回的响应要能被序列化，所以这 4 个都加上。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub id: u8,
    pub symbol: String,
    pub revealed: bool,
}

/// 一张刮刮卡的完整状态（只在服务器内存里存在，不会发给前端）。
///
/// 字段含义：
/// - `id`：卡片的唯一编号，32 位十六进制字符串，由操作系统随机数生成
/// - `cells`：9 个格子，卡片的核心数据
/// - `reward`：这张卡中了多少分。没中奖就是 0
/// - `tier_name`：中奖的奖级名称（"一等奖"等）。没中奖就是 None
/// - `win_symbol`：中奖的符号（💎 等）。没中奖就是 None
/// - `finished`：是否已经结算。结算后就不能再刮也不能再领了
/// - `created_at`：创建时间。服务器用来在卡片太多时清理最旧的卡片
///
/// 关于 `Option<String>`：
/// Rust 里没有 null。想表达"可能没有"就用 Option，它有两个值：
/// - `Some(值)`：有值
/// - `None`：没有值
/// 没中奖的卡，tier_name 就是 None。
///
/// 关于 `std::time::Instant`：
/// Rust 自带的"记录某个时刻"的类型，专门用来比较时间间隔，
/// 和钟表上的具体几点几分无关，只关心"过了多久"。
#[derive(Debug, Clone)]
pub struct Game {
    pub id: String,
    pub cells: Vec<Cell>,
    pub reward: u32,
    pub tier_name: Option<String>,
    pub win_symbol: Option<String>,
    pub finished: bool,
    pub created_at: std::time::Instant,
}

/// 游戏中可能出现的错误，用枚举（enum）把错误分类。
///
/// 什么是枚举：列出"这件事可能出现的几种情况"。
/// 比如游戏错误只可能有下面这 4 种，就列出来，用的时候一个一个匹配。
/// 好处是编译器能帮你检查：有没有把每种情况都处理到。
///
/// - `CardNotFound`：找不到这张卡（可能没创建，也可能被清理掉了）
/// - `CellOutOfRange`：格子编号超出 0~8 的范围
/// - `AlreadyRevealed`：这格已经刮过了，不能重复刮
/// - `AlreadyFinished`：卡已经结算了，不能再操作
///
/// `#[derive(...)]` 里的 PartialEq 和 Eq 让错误之间可以互相比较，
/// 测试里才能写 `assert_eq!(game.reveal_cell(9).unwrap_err(), GameError::CellOutOfRange)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameError {
    CardNotFound,
    CellOutOfRange,
    AlreadyRevealed,
    AlreadyFinished,
}

/// 新建卡片时，返回给前端的"格子提示"。
/// 注意：只告诉前端格子的编号，**不告诉图案**。
/// 图案要等用户真的刮开那一格，调 /api/game/reveal 才返回。
/// 这样中奖结果就完全掌握在服务器手里。
#[derive(Debug, Serialize)]
pub struct CellHint {
    pub id: u8,
}

/// 新建卡片的响应报文（对应接口 POST /api/game/new）。
/// 返回卡片 id + 9 个格子编号。
#[derive(Debug, Serialize)]
pub struct NewCardResponse {
    pub card_id: String,
    pub cells: Vec<CellHint>,
}

/// 刮开一格时前端发来的请求（对应接口 POST /api/game/reveal）。
/// 告诉服务器：哪张卡、刮第几格。
///
/// 这里只用 `Deserialize` 不用 `Serialize`：
/// 因为这个结构体只会"从前端请求里读进来"，永远不会"发回给前端"。
#[derive(Debug, Deserialize)]
pub struct RevealRequest {
    pub card_id: String,
    pub cell_id: u8,
}

/// 刮开一格的响应：这一格是什么图案。
#[derive(Debug, Serialize)]
pub struct RevealResponse {
    pub cell_id: u8,
    pub symbol: String,
}

/// 结算请求（对应接口 POST /api/game/finish）。
/// 只需要告诉服务器是哪张卡，剩下的（中没中奖）由服务器自己查。
#[derive(Debug, Deserialize)]
pub struct FinishRequest {
    pub card_id: String,
}

/// 结算响应：这张卡最终的中奖结果。
/// - `win`：是否中奖（true / false）
/// - `reward`：中奖积分，没中就是 0
/// - `tier`：奖级名称，没中就是 None
/// - `symbol`：中奖符号，没中就是 None
/// - `message`：直接展示给用户的一句话（"恭喜中奖：一等奖 +1000 积分！"）
#[derive(Debug, Serialize)]
pub struct FinishResponse {
    pub win: bool,
    pub reward: u32,
    pub tier: Option<String>,
    pub symbol: Option<String>,
    pub message: String,
}
